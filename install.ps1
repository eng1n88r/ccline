# ccline installer for Windows:
#   irm https://raw.githubusercontent.com/eng1n88r/ccline/master/install.ps1 | iex
#
# Downloads the prebuilt binary from the latest GitHub release into
# %LOCALAPPDATA%\ccline and points Claude Code's statusLine at it.
$ErrorActionPreference = "Stop"

$repo = "eng1n88r/ccline"
$binDir = Join-Path $env:LOCALAPPDATA "ccline"
New-Item -ItemType Directory -Force -Path $binDir | Out-Null

$zip = Join-Path $env:TEMP "ccline.zip"
$url = "https://github.com/$repo/releases/latest/download/ccline-x86_64-pc-windows-msvc.zip"
Write-Host "downloading $url"
Invoke-WebRequest -Uri $url -OutFile $zip
Expand-Archive -Path $zip -DestinationPath $binDir -Force
Remove-Item $zip

$exe = Join-Path $binDir "ccline.exe"

# Point Claude Code's statusLine at ccline, preserving all other settings.
$settingsPath = Join-Path $env:USERPROFILE ".claude\settings.json"
New-Item -ItemType Directory -Force -Path (Split-Path $settingsPath) | Out-Null
if (Test-Path $settingsPath) {
    $settings = Get-Content $settingsPath -Raw | ConvertFrom-Json
} else {
    $settings = [pscustomobject]@{}
}
$old = $settings.statusLine
$statusLine = [pscustomobject]@{
    type            = "command"
    command         = $exe
    padding         = if ($null -ne $old.padding) { $old.padding } else { 0 }
    refreshInterval = if ($null -ne $old.refreshInterval) { $old.refreshInterval } else { 10 }
}
$settings | Add-Member -NotePropertyName statusLine -NotePropertyValue $statusLine -Force
$settings | ConvertTo-Json -Depth 32 | Set-Content $settingsPath
Write-Host "statusLine configured in $settingsPath"
Write-Host "ccline installed to $exe"
