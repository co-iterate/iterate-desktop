// 配对二维码是 iterate:// 自定义 scheme，没装 App 的 iPhone 扫了无处可去（P-2026-9332），
// 所以向导在出配对码之前先给 App Store 入口。用户确认装过一次后本机记住，不再拦。

export const IPHONE_APP_STORE_URL = 'https://apps.apple.com/cn/app/iterate-remote/id6796915922'

/** 用户在安装页停留期间配对码一直在倒计时；剩余不足这么多秒时，确认后先换一张新码 */
export const MIN_SECONDS_LEFT_AFTER_INSTALL = 60

const APP_INSTALLED_STORAGE_KEY = 'iterate.mobileAppInstalled.v1'

export function readAppInstallConfirmed(): boolean {
  try {
    return localStorage.getItem(APP_INSTALLED_STORAGE_KEY) === 'yes'
  }
  catch {
    return false
  }
}

export function writeAppInstallConfirmed() {
  try {
    localStorage.setItem(APP_INSTALLED_STORAGE_KEY, 'yes')
  }
  catch {
    // 存不下时下次打开向导会再显示一次安装页，不影响配对
  }
}

export function needsFreshPairingAfterInstall(remainingSeconds: number) {
  return remainingSeconds < MIN_SECONDS_LEFT_AFTER_INSTALL
}
