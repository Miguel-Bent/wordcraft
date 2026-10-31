# UI smoke test for the tablet app: installs the debug APK, starts it, saves a screenshot of the
# screen, and fails if the app logged a panic, a fatal error or a UI stall (the watchdog in
# apps/wordcraft-tablet logs a freeze longer than 3 s).
#
# Needs: the phone connected with USB debugging on, adb from the SDK platform-tools, and the APK
# built (android/gradlew assembleDebug, see android/README.md).
# Usage (from the project root):
#   powershell -ExecutionPolicy Bypass -File scripts\ui-smoke-android.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\ui-smoke-android.ps1 -Serial <device-id> -Out <folder>
param(
    [string]$Serial = "",
    [string]$Apk = "android\app\build\outputs\apk\debug\app-debug.apk",
    [string]$Out = "target\ui-smoke"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$sdk = Join-Path $env:LOCALAPPDATA "Android\Sdk"
$adb = Join-Path $sdk "platform-tools\adb.exe"
$adbArgs = @()
if ($Serial -ne "") { $adbArgs = @("-s", $Serial) }
$package = "wordcraft.android"
$activity = "$package/.WordCraftActivity"

if (-not (Test-Path $Apk)) { throw "APK not found at $Apk. Build it first (see android/README.md)." }
New-Item -ItemType Directory -Force -Path $Out | Out-Null

Write-Host "1/4 Installing $Apk..."
& $adb @adbArgs install -r $Apk
if ($LASTEXITCODE -ne 0) { throw "adb install failed." }

Write-Host "2/4 Starting the app..."
& $adb @adbArgs logcat -c
& $adb @adbArgs shell am force-stop $package
& $adb @adbArgs shell am start -W -n $activity | Out-Null
Start-Sleep -Seconds 8

Write-Host "3/4 Saving a screenshot of the screen..."
$shot = Join-Path $Out "screen.png"
cmd /c "`"$adb`" $($adbArgs -join ' ') exec-out screencap -p > `"$shot`""
if (-not (Test-Path $shot) -or (Get-Item $shot).Length -lt 1000) { throw "screenshot failed." }

Write-Host "4/4 Checking the log..."
$log = & $adb @adbArgs logcat -d -s wordcraft:* 2>$null
$log | Set-Content -Encoding utf8 (Join-Path $Out "logcat.txt")
$faults = $log | Select-String -Pattern "panicked|FATAL|stalled in"
$fonts = $log | Select-String -Pattern "cataloged (\d+) system font faces" | Select-Object -Last 1
if ($fonts) { Write-Host "System fonts: $($fonts.Matches[0].Groups[1].Value) faces." }

if ($faults) {
    $faults | Select-Object -First 10 | ForEach-Object { Write-Host $_.Line }
    throw "The app logged $($faults.Count) fault(s). See $Out\logcat.txt."
}
Write-Host "OK: no panic, fatal error or UI stall. Screenshot: $shot"
