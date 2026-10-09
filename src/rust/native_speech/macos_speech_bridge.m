#import <AVFoundation/AVFoundation.h>
#import <AppKit/AppKit.h>
#import <Foundation/Foundation.h>
#import <Speech/Speech.h>
#import <objc/message.h>
#import <stdatomic.h>
#include "macos_speech_abi.h"
#include <string.h>

static NSString *SpeechErrorDiagnostic(NSString *stage, NSError *error) {
    NSArray<NSString *> *domains = @[@"kAFAssistantErrorDomain", @"SFSpeechErrorDomain",
        @"NSOSStatusErrorDomain", @"AVFoundationErrorDomain", @"NSCocoaErrorDomain"];
    NSString *domain = error == nil ? @"none" :
        ([domains containsObject:error.domain] ? error.domain : @"other");
    return [NSString stringWithFormat:@"stage=%@ domain=%@ code=%ld",
        stage, domain, (long)(error == nil ? 0 : error.code)];
}

bool speech_bridge_copy_current_bundle_identifier(char *bundleId, size_t capacity) {
    if (bundleId == NULL || capacity == 0) {
        return false;
    }
    @autoreleasepool {
        NSString *identifier = NSBundle.mainBundle.bundleIdentifier;
        const char *utf8 = identifier.UTF8String;
        if (identifier.length == 0 || utf8 == NULL || strlen(utf8) + 1 > capacity) {
            return false;
        }
        memcpy(bundleId, utf8, strlen(utf8) + 1);
        return true;
    }
}

bool speech_bridge_copy_frontmost_application(char *bundleId,
                                              size_t bundleIdCapacity,
                                              int32_t *pid) {
    if (bundleId == NULL || bundleIdCapacity == 0 || pid == NULL) {
        return false;
    }
    __block BOOL copied = NO;
    void (^capture)(void) = ^{
        NSRunningApplication *application = NSWorkspace.sharedWorkspace.frontmostApplication;
        NSString *identifier = application.bundleIdentifier;
        if (application == nil || identifier.length == 0) {
            return;
        }
        const char *utf8 = identifier.UTF8String;
        if (utf8 == NULL || strlen(utf8) + 1 > bundleIdCapacity) {
            return;
        }
        memcpy(bundleId, utf8, strlen(utf8) + 1);
        *pid = application.processIdentifier;
        copied = YES;
    };
    if (NSThread.isMainThread) {
        capture();
    } else {
        dispatch_sync(dispatch_get_main_queue(), capture);
    }
    return copied;
}

bool speech_bridge_activate_application(const char *bundleId, int32_t pid) {
    if (bundleId == NULL) {
        return false;
    }
    NSString *identifier = [NSString stringWithUTF8String:bundleId];
    if (identifier.length == 0) {
        return false;
    }
    __block BOOL activated = NO;
    void (^activate)(void) = ^{
        NSRunningApplication *application = pid > 0
            ? [NSRunningApplication runningApplicationWithProcessIdentifier:pid]
            : nil;
        activated = application != nil
            && [application.bundleIdentifier isEqualToString:identifier]
            && [application activateWithOptions:0];
    };
    if (NSThread.isMainThread) {
        activate();
    } else {
        dispatch_sync(dispatch_get_main_queue(), activate);
    }
    return activated;
}

bool speech_bridge_application_matches_identity(const char *bundleId, int32_t pid) {
    if (bundleId == NULL || pid <= 0) {
        return false;
    }
    NSString *identifier = [NSString stringWithUTF8String:bundleId];
    if (identifier.length == 0) {
        return false;
    }
    __block BOOL matches = NO;
    void (^validate)(void) = ^{
        NSRunningApplication *application =
            [NSRunningApplication runningApplicationWithProcessIdentifier:pid];
        matches = application != nil
            && !application.terminated
            && [application.bundleIdentifier isEqualToString:identifier];
    };
    if (NSThread.isMainThread) {
        validate();
    } else {
        dispatch_sync(dispatch_get_main_queue(), validate);
    }
    return matches;
}

static _Atomic uint64_t g_speech_generation = 0;

@interface IterateSpeechBridge : NSObject

@property(nonatomic, assign) SpeechBridgeCallback callback;
@property(nonatomic, assign) void *userData;
@property(nonatomic, strong) SFSpeechRecognizer *speechRecognizer;
@property(nonatomic, strong) AVAudioEngine *audioEngine;
@property(nonatomic, strong) SFSpeechAudioBufferRecognitionRequest *recognitionRequest;
@property(nonatomic, strong) SFSpeechRecognitionTask *recognitionTask;
@property(nonatomic, copy) NSArray<NSString *> *contextualStrings;
@property(nonatomic, assign) BOOL hasInputTap;
@property(nonatomic, assign) BOOL finishingRecognition;
@property(nonatomic, assign) uint64_t generation;
@property(nonatomic, assign) SpeechLayerIdentity identity;
@property(nonatomic, assign) NSUInteger stopGeneration;
@property(nonatomic, assign) BOOL forceOnDeviceRecognition;

+ (instancetype)shared;
- (void)startWithCallback:(SpeechBridgeCallback)callback
                 userData:(void *)userData
                 identity:(SpeechLayerIdentity)identity
        contextualStrings:(NSArray<NSString *> *)contextualStrings
 forceOnDeviceRecognition:(BOOL)forceOnDeviceRecognition
               generation:(uint64_t)generation;
- (BOOL)acceptsGeneration:(uint64_t)generation identity:(SpeechLayerIdentity)identity;

@end

@implementation IterateSpeechBridge

+ (instancetype)shared {
    static IterateSpeechBridge *shared = nil;
    static dispatch_once_t onceToken;
    dispatch_once(&onceToken, ^{
        shared = [[IterateSpeechBridge alloc] init];
    });
    return shared;
}

- (instancetype)init {
    self = [super init];
    if (self) {
        _audioEngine = [[AVAudioEngine alloc] init];
        _speechRecognizer =
            [[SFSpeechRecognizer alloc] initWithLocale:[[NSLocale alloc] initWithLocaleIdentifier:@"zh-CN"]];
    }
    return self;
}

- (BOOL)acceptsGeneration:(uint64_t)generation identity:(SpeechLayerIdentity)identity {
    return self.generation == generation &&
           atomic_load(&g_speech_generation) == generation &&
           SpeechLayerIdentityEqual(self.identity, identity);
}

- (void)emitEvent:(NSString *)eventType
             text:(NSString *)text
         identity:(SpeechLayerIdentity)identity
       generation:(uint64_t)generation {
    if (self.callback == NULL || ![self acceptsGeneration:generation identity:identity]) {
        return;
    }

    const char *eventCString = [eventType UTF8String];
    const char *textCString = text != nil ? [text UTF8String] : "";
    self.callback(identity, eventCString, textCString, self.userData);
}

- (void)resetRecognition:(BOOL)cancelTask {
    self.finishingRecognition = NO;
    self.stopGeneration += 1;

    if (self.audioEngine.isRunning) {
        [self.audioEngine stop];
    }

    if (self.hasInputTap) {
        [[self.audioEngine inputNode] removeTapOnBus:0];
        self.hasInputTap = NO;
    }

    [self.recognitionRequest endAudio];
    self.recognitionRequest = nil;

    if (cancelTask) {
        [self.recognitionTask cancel];
    }
    self.recognitionTask = nil;
    self.audioEngine = [[AVAudioEngine alloc] init];
}

- (void)resetRecognitionForGeneration:(uint64_t)generation
                              identity:(SpeechLayerIdentity)identity
                            cancelTask:(BOOL)cancelTask {
    if (![self acceptsGeneration:generation identity:identity]) {
        return;
    }
    [self resetRecognition:cancelTask];
}

- (void)finishAudioInputAndWaitForFinal {
    if (self.recognitionRequest == nil && self.recognitionTask == nil && !self.audioEngine.isRunning) {
        [self resetRecognition:NO];
        return;
    }

    self.finishingRecognition = YES;
    self.stopGeneration += 1;
    NSUInteger stopGeneration = self.stopGeneration;
    uint64_t generation = self.generation;
    SpeechLayerIdentity finishIdentity = self.identity;

    if (self.audioEngine.isRunning) {
        [self.audioEngine stop];
    }

    if (self.hasInputTap) {
        [[self.audioEngine inputNode] removeTapOnBus:0];
        self.hasInputTap = NO;
    }

    [self.recognitionRequest endAudio];
    self.recognitionRequest = nil;
    [self.recognitionTask finish];

    __weak typeof(self) weakSelf = self;
    dispatch_after(dispatch_time(DISPATCH_TIME_NOW, (int64_t)(3.0 * NSEC_PER_SEC)),
                   dispatch_get_main_queue(), ^{
                       __strong typeof(weakSelf) strongSelf = weakSelf;
                       if (strongSelf == nil) {
                           return;
                       }
                       if (strongSelf.finishingRecognition &&
                           strongSelf.stopGeneration == stopGeneration &&
                           SpeechLayerIdentityEqual(strongSelf.identity, finishIdentity) &&
                           [strongSelf acceptsGeneration:generation identity:finishIdentity]) {
                           [strongSelf resetRecognitionForGeneration:generation
                                                            identity:finishIdentity
                                                          cancelTask:YES];
                       }
                   });
}

- (void)beginRecognitionForGeneration:(uint64_t)generation
                              identity:(SpeechLayerIdentity)identity {
    [self resetRecognition:YES];

    if (self.speechRecognizer == nil || !self.speechRecognizer.isAvailable) {
        [self emitEvent:@"diag-error" text:SpeechErrorDiagnostic(@"recognizer-unavailable", nil)
               identity:identity generation:generation];
        [self emitEvent:@"error"
                   text:@"macOS 语音识别当前不可用"
               identity:identity
             generation:generation];
        return;
    }

    NSLog(@"[iterate-speech-bridge] beginRecognition: recognizer=%@ available=%d",
          self.speechRecognizer.locale.localeIdentifier,
          self.speechRecognizer.isAvailable);

    self.recognitionRequest = [[SFSpeechAudioBufferRecognitionRequest alloc] init];
    self.recognitionRequest.shouldReportPartialResults = YES;
    self.recognitionRequest.taskHint = SFSpeechRecognitionTaskHintDictation;

    SEL addsPunctuationSelector = NSSelectorFromString(@"setAddsPunctuation:");
    if ([self.recognitionRequest respondsToSelector:addsPunctuationSelector]) {
        ((void (*)(id, SEL, BOOL))objc_msgSend)(self.recognitionRequest, addsPunctuationSelector, NO);
    }

    if (self.contextualStrings.count > 0 &&
        [self.recognitionRequest respondsToSelector:@selector(setContextualStrings:)]) {
        self.recognitionRequest.contextualStrings = self.contextualStrings;
    }

    NSString *recognitionMode = self.forceOnDeviceRecognition ? @"privacy-unavailable-fallback" : @"quality";
    SEL supportsOnDeviceSelector = NSSelectorFromString(@"supportsOnDeviceRecognition");
    SEL requiresOnDeviceSelector = NSSelectorFromString(@"setRequiresOnDeviceRecognition:");
    if (self.forceOnDeviceRecognition &&
        [self.speechRecognizer respondsToSelector:supportsOnDeviceSelector] &&
        [self.recognitionRequest respondsToSelector:requiresOnDeviceSelector]) {
        BOOL supportsOnDevice =
            ((BOOL (*)(id, SEL))objc_msgSend)(self.speechRecognizer, supportsOnDeviceSelector);
        if (supportsOnDevice) {
            ((void (*)(id, SEL, BOOL))objc_msgSend)(self.recognitionRequest, requiresOnDeviceSelector, YES);
            recognitionMode = @"privacy-on-device";
        }
    }

    AVAudioInputNode *inputNode = [self.audioEngine inputNode];
    AVAudioFormat *recordingFormat = [inputNode outputFormatForBus:0];

    [self emitEvent:@"diag-audio-format"
               text:[NSString stringWithFormat:@"sampleRate=%.0f channels=%u",
                     recordingFormat.sampleRate, recordingFormat.channelCount]
           identity:identity
         generation:generation];

    __weak typeof(self) weakSelf = self;
    __block NSUInteger tapCount = 0;
    [inputNode installTapOnBus:0
                    bufferSize:256
                        format:recordingFormat
                         block:^(AVAudioPCMBuffer *buffer, AVAudioTime *when) {
                             (void)when;
                             __strong typeof(weakSelf) strongSelf = weakSelf;
                             if (strongSelf == nil || strongSelf.recognitionRequest == nil ||
                                 ![strongSelf acceptsGeneration:generation identity:identity]) {
                                 return;
                             }
                             tapCount++;
                             if (tapCount == 1 || tapCount == 10 || tapCount == 50) {
                                 [strongSelf emitEvent:@"diag-tap-buffer"
                                                  text:[NSString stringWithFormat:@"count=%lu frames=%u",
                                                        (unsigned long)tapCount, buffer.frameLength]
                                              identity:identity
                                            generation:generation];
                             }
                             [strongSelf.recognitionRequest appendAudioPCMBuffer:buffer];
                         }];
    self.hasInputTap = YES;

    self.recognitionTask =
        [self.speechRecognizer recognitionTaskWithRequest:self.recognitionRequest
                                            resultHandler:^(SFSpeechRecognitionResult *result, NSError *error) {
                                                __strong typeof(weakSelf) strongSelf = weakSelf;
                                                SpeechLayerIdentity capturedIdentity = identity;
                                                if (strongSelf == nil) {
                                                    return;
                                                }
                                                if (![strongSelf acceptsGeneration:generation identity:capturedIdentity]) {
                                                    return;
                                                }

                                                if (result != nil) {
                                                    NSString *bestTranscript =
                                                        result.bestTranscription.formattedString ?: @"";
                                                    [strongSelf emitEvent:(result.isFinal ? @"final" : @"partial")
                                                                     text:bestTranscript
                                                                 identity:capturedIdentity
                                                               generation:generation];
                                                    if (result.isFinal) {
                                                        [strongSelf resetRecognitionForGeneration:generation
                                                                                         identity:capturedIdentity
                                                                                       cancelTask:NO];
                                                        return;
                                                    }
                                                }

                                                if (error != nil) {
                                                    [strongSelf emitEvent:@"diag-error"
                                                                     text:SpeechErrorDiagnostic(@"recognition-task", error)
                                                                 identity:capturedIdentity generation:generation];
                                                    NSString *message = error.localizedDescription ?: @"未知错误";
                                                    [strongSelf emitEvent:@"error"
                                                                     text:message
                                                                 identity:capturedIdentity
                                                               generation:generation];
                                                    [strongSelf resetRecognitionForGeneration:generation
                                                                                     identity:capturedIdentity
                                                                                   cancelTask:YES];
                                                }
                                            }];

    NSError *startError = nil;
    [self.audioEngine prepare];
    if (![self.audioEngine startAndReturnError:&startError]) {
        [self emitEvent:@"diag-error" text:SpeechErrorDiagnostic(@"audio-engine-start", startError)
               identity:identity generation:generation];
        NSString *message = startError.localizedDescription ?: @"音频引擎启动失败";
        [self emitEvent:@"error" text:message identity:identity generation:generation];
        [self resetRecognition:YES];
        return;
    }

    NSLog(@"[iterate-speech-bridge] engine started successfully, mode=%@, isRunning=%d",
          recognitionMode, self.audioEngine.isRunning);
    [self emitEvent:@"started" text:recognitionMode identity:identity generation:generation];
}

- (void)startWithCallback:(SpeechBridgeCallback)callback
                 userData:(void *)userData
                 identity:(SpeechLayerIdentity)identity
        contextualStrings:(NSArray<NSString *> *)contextualStrings
 forceOnDeviceRecognition:(BOOL)forceOnDeviceRecognition
               generation:(uint64_t)generation {
    [self resetRecognition:YES];
    self.callback = callback;
    self.userData = userData;
    self.identity = identity;
    self.contextualStrings = contextualStrings ?: @[];
    self.forceOnDeviceRecognition = forceOnDeviceRecognition;
    self.generation = generation;

    __weak typeof(self) weakSelf = self;
    [SFSpeechRecognizer requestAuthorization:^(SFSpeechRecognizerAuthorizationStatus status) {
        dispatch_async(dispatch_get_main_queue(), ^{
            __strong typeof(weakSelf) strongSelf = weakSelf;
            if (strongSelf == nil) {
                return;
            }
            if (strongSelf.generation != generation || atomic_load(&g_speech_generation) != generation ||
                !SpeechLayerIdentityEqual(strongSelf.identity, identity)) {
                return;
            }

            if (status != SFSpeechRecognizerAuthorizationStatusAuthorized) {
                [strongSelf emitEvent:@"diag-error" text:SpeechErrorDiagnostic(@"speech-authorization", nil)
                             identity:identity generation:generation];
                [strongSelf emitEvent:@"error"
                                 text:@"语音识别权限未开启"
                             identity:identity
                           generation:generation];
                return;
            }

            [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio
                                     completionHandler:^(BOOL granted) {
                                         dispatch_async(dispatch_get_main_queue(), ^{
                                             __strong typeof(weakSelf) innerSelf = weakSelf;
                                             if (innerSelf == nil) {
                                                 return;
                                             }
                                             if (innerSelf.generation != generation ||
                                                 atomic_load(&g_speech_generation) != generation ||
                                                 !SpeechLayerIdentityEqual(innerSelf.identity, identity)) {
                                                 return;
                                             }

                                             if (!granted) {
                                                 [innerSelf emitEvent:@"diag-error" text:SpeechErrorDiagnostic(@"microphone-authorization", nil)
                                                             identity:identity generation:generation];
                                                 [innerSelf emitEvent:@"error"
                                                                 text:@"麦克风权限未开启"
                                                             identity:identity
                                                           generation:generation];
                                                 return;
                                             }

                                             [innerSelf beginRecognitionForGeneration:generation
                                                                             identity:identity];
                                         });
                                     }];
        });
    }];
}

@end

void speech_bridge_start(SpeechLayerIdentity identity,
                         SpeechBridgeCallback callback,
                         void *user_data,
                         const char **contextual_strings,
                         size_t contextual_count,
                         bool force_on_device) {
    NSMutableArray<NSString *> *phrases = [NSMutableArray arrayWithCapacity:contextual_count];
    if (contextual_strings != NULL) {
        NSCharacterSet *trimSet = [NSCharacterSet whitespaceAndNewlineCharacterSet];
        for (size_t i = 0; i < contextual_count && phrases.count < 100; i++) {
            const char *rawPhrase = contextual_strings[i];
            if (rawPhrase == NULL) {
                continue;
            }
            NSString *phrase =
                [[NSString stringWithUTF8String:rawPhrase] stringByTrimmingCharactersInSet:trimSet];
            if (phrase.length > 0) {
                [phrases addObject:phrase];
            }
        }
    }

    NSArray<NSString *> *contextualSnapshot = [phrases copy];
    uint64_t mine = atomic_fetch_add(&g_speech_generation, 1) + 1;
    dispatch_async(dispatch_get_main_queue(), ^{
        uint64_t current = atomic_load(&g_speech_generation);
        if (mine != current) {
            return;
        }
        [[IterateSpeechBridge shared] startWithCallback:callback
                                              userData:user_data
                                              identity:identity
                                     contextualStrings:contextualSnapshot
                              forceOnDeviceRecognition:(force_on_device ? YES : NO)
                                            generation:mine];
    });
}

void speech_bridge_finish(SpeechLayerIdentity identity) {
    void (^finishBlock)(void) = ^{
        IterateSpeechBridge *bridge = [IterateSpeechBridge shared];
        if (![bridge acceptsGeneration:bridge.generation identity:identity]) {
            return;
        }
        [bridge finishAudioInputAndWaitForFinal];
    };
    if ([NSThread isMainThread]) {
        finishBlock();
    } else {
        dispatch_sync(dispatch_get_main_queue(), finishBlock);
    }
}

void speech_bridge_cancel(SpeechLayerIdentity identity) {
    void (^cancelBlock)(void) = ^{
        IterateSpeechBridge *bridge = [IterateSpeechBridge shared];
        uint64_t generation = bridge.generation;
        if (![bridge acceptsGeneration:generation identity:identity]) {
            return;
        }
        atomic_fetch_add(&g_speech_generation, 1);
        [bridge resetRecognition:YES];
    };
    if ([NSThread isMainThread]) {
        cancelBlock();
    } else {
        dispatch_sync(dispatch_get_main_queue(), cancelBlock);
    }
}

bool speech_bridge_main_bundle_has_usage_description(const char *usage_key) {
    if (usage_key == NULL) {
        return false;
    }

    NSString *key = [NSString stringWithUTF8String:usage_key];
    if (key == nil || key.length == 0) {
        return false;
    }

    id value = [[NSBundle mainBundle] objectForInfoDictionaryKey:key];
    return [value isKindOfClass:[NSString class]] && [(NSString *)value length] > 0;
}

bool speech_bridge_check_microphone_authorization(void) {
    return [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio] == AVAuthorizationStatusAuthorized;
}

bool speech_bridge_request_microphone_authorization(void) {
    AVAuthorizationStatus status = [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio];
    if (status != AVAuthorizationStatusNotDetermined) {
        return status == AVAuthorizationStatusAuthorized;
    }

    __block BOOL granted = NO;
    dispatch_semaphore_t semaphore = dispatch_semaphore_create(0);

    void (^requestBlock)(void) = ^{
        [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio
                                 completionHandler:^(BOOL didGrant) {
                                     granted = didGrant;
                                     dispatch_semaphore_signal(semaphore);
                                 }];
    };

    if ([NSThread isMainThread]) {
        requestBlock();
        return [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio] == AVAuthorizationStatusAuthorized;
    }

    dispatch_async(dispatch_get_main_queue(), requestBlock);
    long waitResult = dispatch_semaphore_wait(semaphore, dispatch_time(DISPATCH_TIME_NOW, 60 * NSEC_PER_SEC));
    if (waitResult != 0) {
        return [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio] == AVAuthorizationStatusAuthorized;
    }
    return granted;
}

bool speech_bridge_check_speech_authorization(void) {
    return [SFSpeechRecognizer authorizationStatus] == SFSpeechRecognizerAuthorizationStatusAuthorized;
}

bool speech_bridge_request_speech_authorization(void) {
    SFSpeechRecognizerAuthorizationStatus status = [SFSpeechRecognizer authorizationStatus];
    if (status != SFSpeechRecognizerAuthorizationStatusNotDetermined) {
        return status == SFSpeechRecognizerAuthorizationStatusAuthorized;
    }

    __block SFSpeechRecognizerAuthorizationStatus resolvedStatus = status;
    dispatch_semaphore_t semaphore = dispatch_semaphore_create(0);

    void (^requestBlock)(void) = ^{
        [SFSpeechRecognizer requestAuthorization:^(SFSpeechRecognizerAuthorizationStatus nextStatus) {
            resolvedStatus = nextStatus;
            dispatch_semaphore_signal(semaphore);
        }];
    };

    if ([NSThread isMainThread]) {
        requestBlock();
        return [SFSpeechRecognizer authorizationStatus] == SFSpeechRecognizerAuthorizationStatusAuthorized;
    }

    dispatch_async(dispatch_get_main_queue(), requestBlock);
    long waitResult = dispatch_semaphore_wait(semaphore, dispatch_time(DISPATCH_TIME_NOW, 60 * NSEC_PER_SEC));
    if (waitResult != 0) {
        return [SFSpeechRecognizer authorizationStatus] == SFSpeechRecognizerAuthorizationStatusAuthorized;
    }
    return resolvedStatus == SFSpeechRecognizerAuthorizationStatusAuthorized;
}
