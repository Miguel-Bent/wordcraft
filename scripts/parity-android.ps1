# Parity test: the same engine checks run on this PC and on an Android phone; the outputs must match.
#
# Needs: the Android phone connected with USB debugging on (or an emulator running), and adb
# from the SDK platform-tools. Usage (from the project root):
#   powershell -ExecutionPolicy Bypass -File scripts\parity-android.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\parity-android.ps1 -Serial <device-id>
param(
    [string]$Serial = ""
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

$sdk = Join-Path $env:LOCALAPPDATA "Android\Sdk"
$env:ANDROID_NDK_HOME = Join-Path $sdk "ndk\27.2.12479018"
$adb = Join-Path $sdk "platform-tools\adb.exe"
$adbArgs = @()
if ($Serial -ne "") { $adbArgs = @("-s", $Serial) }

Write-Host "1/4 Building the parity check for Android (arm64-v8a)..."
cargo ndk -t arm64-v8a build -p wordcraft-android --example parity --release
if ($LASTEXITCODE -ne 0) { throw "Android build failed." }

# Both sides scan only the bundled fonts: the phone and the PC have different installed fonts, and
# a font the document asks for is then substituted differently. The folder below doesn't exist.
$env:WORDCRAFT_FONT_DIRS = "C:\wordcraft-no-system-fonts"
$deviceFontDirs = "/data/wordcraft-no-system-fonts"

Write-Host "2/4 Running the same check on this PC..."
# cmd drops cargo's stderr (build progress and warnings); PowerShell would turn it into errors.
cmd /c "cargo run -p wordcraft-android --example parity --release 2>nul" | Set-Content -Encoding utf8 parity-host.txt

Write-Host "3/4 Sending it to the phone and running it there..."
& $adb @adbArgs push "target\aarch64-linux-android\release\examples\parity" "/data/local/tmp/parity"
if ($LASTEXITCODE -ne 0) { throw "adb push failed. Is the phone connected and authorised?" }
& $adb @adbArgs shell chmod 755 "/data/local/tmp/parity"
$device = & $adb @adbArgs shell "WORDCRAFT_FONT_DIRS=$deviceFontDirs /data/local/tmp/parity"
($device -replace "`r", "") | Set-Content -Encoding utf8 parity-android.txt

Write-Host "4/4 Comparing the two outputs..."
$host_lines = Get-Content parity-host.txt
$device_lines = Get-Content parity-android.txt
$differences = Compare-Object $host_lines $device_lines
if (-not $differences) {
    Write-Host "IDENTICAL: the PC and the phone give the same results ($($host_lines.Count) lines)."
} else {
    Write-Host "DIFFERENT: $(@($differences).Count) lines differ. See parity-host.txt and parity-android.txt."
    $differences | Select-Object -First 20 | Format-Table -AutoSize
}
