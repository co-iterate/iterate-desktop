import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import test from 'node:test'

function source(path) {
  return readFileSync(new URL(`../${path}`, import.meta.url), 'utf8')
}

test('Windows registry liveness check does not spawn tasklist', () => {
  const registry = source('src/rust/ui/window_registry.rs')
  assert.doesNotMatch(registry, /Command::new\("tasklist"\)/)
  assert.match(registry, /OpenProcess\(PROCESS_QUERY_LIMITED_INFORMATION/)
})

test('bridge health probe uses reqwest only on Windows and preserves the Unix curl path', () => {
  const setup = source('src/rust/app/setup.rs')
  assert.match(setup, /#\[cfg\(target_os = "windows"\)\]\s*fn bridge_http_healthy[\s\S]*?reqwest::blocking::Client::builder/)
  assert.match(setup, /#\[cfg\(not\(target_os = "windows"\)\)\]\s*fn bridge_http_healthy[\s\S]*?command_stdout\("curl"/)
})

test('Windows shows the main window before background setup while non-Windows keeps blocking setup', () => {
  const builder = source('src/rust/app/builder.rs')
  const showIndex = builder.indexOf('window.show()')
  const setupIndex = builder.lastIndexOf('start_application_setup(app_handle)')
  assert.ok(showIndex >= 0 && setupIndex > showIndex)
  assert.match(builder, /#\[cfg\(target_os = "windows"\)\][\s\S]*?start_application_setup\(app_handle\)/)
  assert.match(builder, /#\[cfg\(not\(target_os = "windows"\)\)\][\s\S]*?async_runtime::block_on/)
})

test('Windows speech startup failure degrades without blocking the main application', () => {
  const app = source('src/frontend/App.vue')
  const windowsFallback = /if \(windowsPlatform\) \{\s*try \{\s*await speechRuntimeHost\.initialize\(\)\s*\}\s*catch \(error\) \{([\s\S]*?)\}\s*\}\s*else \{\s*await speechRuntimeHost\.initialize\(\)\s*\}/
  const fallbackMatch = windowsFallback.exec(app)

  assert.ok(fallbackMatch, 'speech initialization should degrade only on Windows')
  assert.match(fallbackMatch[1], /console\.warn\('语音运行时初始化失败，主界面将继续启动:', error\)/)
  assert.match(fallbackMatch[1], /onMounted:speechRuntimeDegraded/)
  assert.doesNotMatch(fallbackMatch[1], /\breturn\b/)

  const mcpShellIndex = app.indexOf('if (mcpLaunchContext.value.isMcp)')
  const mcpInitializeIndex = app.indexOf('await initializeApplication({ mcpShell: true })', mcpShellIndex)
  const mcpReturnIndex = app.indexOf('return', mcpInitializeIndex)
  const speechFallbackIndex = fallbackMatch.index
  const speechFallbackEnd = speechFallbackIndex + fallbackMatch[0].length
  const activationGateIndex = app.indexOf('const activationGateRequired = await requiresActivationGate()', speechFallbackIndex)
  const initializeApplicationIndex = app.indexOf('await initializeApplication()', activationGateIndex)
  const speechInitializeIndexes = [...app.matchAll(/await speechRuntimeHost\.initialize\(\)/g)].map(match => match.index)
  const activationGateCalls = [...app.matchAll(/await requiresActivationGate\(\)/g)]

  assert.ok(mcpShellIndex >= 0 && mcpInitializeIndex > mcpShellIndex, 'MCP shell should initialize the popup application')
  assert.ok(mcpReturnIndex > mcpInitializeIndex && mcpReturnIndex < speechFallbackIndex, 'MCP shell should return before speech startup')
  assert.doesNotMatch(app.slice(mcpShellIndex, mcpReturnIndex), /speechRuntimeHost\.initialize|requiresActivationGate/)
  assert.equal(speechInitializeIndexes.length, 2, 'startup should contain only the Windows and non-Windows speech calls')
  assert.ok(
    speechInitializeIndexes.every(index => index >= speechFallbackIndex && index < speechFallbackEnd),
    'all speech initialization calls should stay inside the platform fallback block',
  )
  assert.equal(activationGateCalls.length, 1, 'startup should perform exactly one activation gate call')
  assert.ok(activationGateIndex > speechFallbackIndex, 'activation checks should continue after degraded speech startup')
  assert.ok(initializeApplicationIndex > activationGateIndex, 'main application initialization should remain reachable')
})

test('window registry cleanup runs off the UI thread at low frequency', () => {
  const events = source('src/rust/ui/window_events.rs')
  const builder = source('src/rust/app/builder.rs')
  assert.match(events, /WINDOW_REGISTRY_CLEANUP_INTERVAL[^;]*Duration::from_secs\(5 \* 60\)/s)
  assert.match(events, /start_window_registry_cleanup_task[\s\S]*spawn_blocking[\s\S]*get_all_instances/)
  assert.match(builder, /start_window_registry_cleanup_task\(\)/)
})

test('close flow exits without recursively closing the window', () => {
  const exit = source('src/rust/ui/exit.rs')
  const lifecycle = source('src/rust/app/windows_lifecycle.rs')
  assert.match(exit, /exit_in_progress\.swap/)
  assert.match(exit, /#\[cfg\(target_os = "windows"\)\][\s\S]*?window\.hide\(\)/)
  assert.match(exit, /#\[cfg\(not\(target_os = "windows"\)\)\][\s\S]*?window\.close\(\)/)
  assert.match(exit, /request_global_shutdown\(\)/)
  assert.match(lifecycle, /CreateEventW/)
  assert.match(lifecycle, /QueryFullProcessImageNameW/)
  assert.match(lifecycle, /GetProcessTimes/)
  assert.match(lifecycle, /OpenProcessToken/)
  assert.match(lifecycle, /GetTokenInformation/)
  assert.match(lifecycle, /TerminateProcess/)
  assert.doesNotMatch(lifecycle, /taskkill|tasklist|Stop-Process|Command::new/)
})

test('frontend and package defaults preserve macOS behavior', () => {
  const app = source('src/frontend/App.vue')
  const content = source('src/frontend/components/AppContent.vue')
  const settings = source('src/frontend/composables/useSettings.ts')
  const pkg = JSON.parse(source('package.json'))
  assert.match(app, /windowsPlatform && startupStatus\.phase !== 'ready'/)
  assert.match(content, /\? 'iterate'\s*: `iterate - \$\{resolvedProjectPath\}`/)
  assert.match(settings, /if \(!windowsPlatform\)[\s\S]*?reloadAllSettings\(\)/)
  assert.equal(pkg.scripts.tauri, 'cargo tauri')
  assert.equal(pkg.scripts['tauri:build'], 'cargo tauri build')
})

test('manual close blocks automatic zhi startup until a shortcut launch clears it', () => {
  const lifecycle = source('src/rust/app/windows_lifecycle.rs')
  const main = source('src/rust/main.rs')
  const mcpServer = source('src/bin/mcp-server.rs')
  assert.match(lifecycle, /args\.len\(\) != 1/)
  assert.match(lifecycle, /remove_file\(manual_stop_path\(\)\)/)
  assert.match(main, /activate_manual_launch_if_requested/)
  assert.match(mcpServer, /is_manually_stopped\(\)/)
  assert.match(mcpServer, /MANUALLY_STOPPED_MESSAGE/)
})

test('explicit conversation end is exact and is normalized at the response boundary', () => {
  const command = source('src/rust/conversation/end_command.rs')
  const cli = source('src/rust/app/cli.rs')
  const interaction = source('src/rust/mcp/tools/interaction/mcp.rs')
  const popup = source('src/frontend/components/popup/PopupInput.vue')
  const frontendCommand = source('src/frontend/utils/conversationEndCommand.ts')
  assert.match(command, /"结束对话"/)
  assert.match(command, /"\/end"/)
  assert.match(command, /eq_ignore_ascii_case/)
  assert.match(command, /selected_options[\s\S]*?is_explicit_conversation_end/)
  assert.match(cli, /keep_going: !interaction_ended/)
  assert.match(cli, /EXPLICIT_CONVERSATION_END_SOURCE/)
  assert.match(cli, /POPUP_CLOSED_SOURCE/)
  assert.match(interaction, /继续对话: false/)
  assert.match(popup, /输入“结束对话”或 \/end 可结束本次交互/)
  assert.match(frontendCommand, /isExplicitConversationEndInput/)
  assert.ok(
    popup.indexOf('isExplicitConversationEndInput(clipboardText)')
    < popup.indexOf('extractClipboardPaths(clipboardText)'),
    'explicit end commands must bypass clipboard path attachment handling',
  )
  assert.match(
    popup,
    /!isExplicitConversationEndInput\(userInput\.value\)[\s\S]*?generateConditionalContent\(\)/,
    'explicit end commands must reach the Rust response boundary without appended context',
  )
})

test('popup close ends only the current interaction while the native titlebar still exits', () => {
  const header = source('src/frontend/components/popup/PopupHeader.vue')
  const content = source('src/frontend/components/AppContent.vue')
  const app = source('src/frontend/App.vue')
  const handler = source('src/frontend/composables/useMcpHandler.ts')
  const windowEvents = source('src/rust/ui/window_events.rs')
  assert.match(header, /closeCurrentDialog/)
  assert.match(header, /结束当前对话（iterate 继续运行）/)
  assert.match(content, /mcpCloseCurrentDialog/)
  assert.match(app, /mcp-close-current-dialog/)
  assert.match(handler, /handleMcpCloseCurrentDialog/)
  assert.match(handler, /source: 'popup_closed'/)
  assert.match(handler, /resolvingRequestIds/)
  assert.match(windowEvents, /handle_system_exit_request[\s\S]*?true/)
})

test('Windows bundle uses a current-user NSIS installer', () => {
  const config = JSON.parse(source('tauri.conf.json'))
  assert.equal(config.bundle.windows.nsis.installMode, 'currentUser')
})

test('Windows package smoke waits for the GUI-subsystem activation probe and captures its output', () => {
  for (const path of [
    'scripts/windows-install-smoke.ps1',
    'scripts/windows-installer-smoke.ps1',
  ]) {
    const smoke = source(path)
    assert.match(smoke, /System\.Diagnostics\.ProcessStartInfo/, path)
    assert.match(smoke, /UseShellExecute\s*=\s*\$false/, path)
    assert.match(smoke, /RedirectStandardOutput\s*=\s*\$true/, path)
    assert.match(smoke, /RedirectStandardError\s*=\s*\$true/, path)
    assert.match(smoke, /WaitForExit\(\)/, path)
    assert.doesNotMatch(smoke, /\$ActivationProbe\s*=\s*&/, path)
  }
})

test('Windows popup shortcuts use the new defaults and safely migrate only the complete legacy set', () => {
  const settings = source('src/rust/config/settings.rs')
  const storage = source('src/rust/config/storage.rs')
  const shortcuts = source('src/frontend/composables/useShortcuts.ts')
  const popupInput = source('src/frontend/components/popup/PopupInput.vue')
  const popupActions = source('src/frontend/components/popup/PopupActions.vue')

  assert.match(settings, /let is_macos = cfg!\(target_os = "macos"\)/)
  assert.match(settings, /quick_submit[\s\S]*?shift: !is_macos,[\s\S]*?meta: is_macos/)
  assert.match(settings, /enhance[\s\S]*?ctrl: !is_macos,[\s\S]*?alt: is_macos,[\s\S]*?shift: !is_macos/)
  assert.match(settings, /continue[\s\S]*?ctrl: !is_macos,[\s\S]*?shift: is_macos/)

  assert.match(storage, /fn has_complete_legacy_popup_defaults/)
  assert.match(storage, /if has_complete_legacy_popup_defaults\(config\)/)
  assert.match(storage, /for key in \["quick_submit", "continue", "enhance"\]/)
  assert.match(storage, /if !config\.shortcut_config\.shortcuts\.contains_key\(&key\)/)
  assert.match(storage, /merge_migrates_only_the_complete_legacy_popup_default_set/)
  assert.match(storage, /merge_preserves_the_whole_set_when_one_legacy_binding_was_customized/)
  assert.doesNotMatch(storage, /is_old_ctrl_shift|is_old_ctrl|is_old_shift|is_old_alt/)

  assert.match(shortcuts, /event\.isComposing/)
  assert.match(shortcuts, /event\.keyCode === 229/)
  assert.match(shortcuts, /event\.repeat/)
  assert.match(shortcuts, /document\.visibilityState !== 'hidden' && document\.hasFocus\(\)/)
  assert.match(shortcuts, /event\.ctrlKey === shortcutKey\.ctrl[\s\S]*?event\.shiftKey === shortcutKey\.shift/)
  assert.match(popupInput, /isComposing\.value \|\| event\.isComposing \|\| event\.keyCode === 229 \|\| event\.repeat/)

  assert.match(popupActions, /props\.canSubmit && !props\.submitting[\s\S]*?handleSubmit\(\)/)
  assert.match(popupActions, /props\.canSubmit && !props\.submitting[\s\S]*?handleGoalSubmit\(\)/)
  assert.match(popupActions, /!props\.submitting[\s\S]*?handleContinue\(\)/)
})

test('Windows popup screenshot keeps the Mac capability with a native Windows capture path', () => {
  const setup = source('src/rust/app/setup.rs')
  const commands = source('src/rust/ui/commands.rs')
  const input = source('src/frontend/components/popup/PopupInput.vue')
  const cargo = source('Cargo.toml')
  assert.match(setup, /#\[cfg\(target_os = "macos"\)\][\s\S]*?Shift\+Cmd\+K/)
  assert.doesNotMatch(setup, /Shift\+Ctrl\+K/)
  assert.match(setup, /capture_screenshot\(\)\.await/)
  assert.match(input, /async function handleWindowsScreenshotShortcut\(event: KeyboardEvent\)/)
  assert.match(input, /event\.code !== 'F8'[\s\S]*?event\.ctrlKey[\s\S]*?event\.altKey[\s\S]*?event\.shiftKey[\s\S]*?event\.metaKey/)
  assert.match(input, /await webview\.hide\(\)[\s\S]*?invoke<string>\('capture_screenshot'\)[\s\S]*?await webview\.show\(\)[\s\S]*?await webview\.setFocus\(\)/)
  assert.match(input, /window\.addEventListener\('keydown', handleWindowsScreenshotShortcut\)/)
  assert.match(input, /window\.removeEventListener\('keydown', handleWindowsScreenshotShortcut\)/)
  assert.match(commands, /#\[cfg\(target_os = "windows"\)\][\s\S]*?CreateDIBSection[\s\S]*?BitBlt/)
  assert.match(commands, /PngEncoder::new[\s\S]*?ExtendedColorType::Rgba8/)
  assert.doesNotMatch(commands, /iterate_screenshot_[\s\S]*?System\.Windows\.Forms/)
  assert.match(cargo, /"Win32_Graphics_Gdi"/)
  assert.match(cargo, /"Win32_UI_WindowsAndMessaging"/)
  assert.match(cargo, /image = \{ version = "0\.25\.10"/)
})
