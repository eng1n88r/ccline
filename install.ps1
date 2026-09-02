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

# Claude Code runs the statusLine command through Git Bash when Git for Windows
# is installed, and bash eats backslashes ("C:UsersfooCcline.exe: not found"),
# so hand it a forward-slash path — which both bash and PowerShell accept.
# Neither shell tolerates an unquoted space, so fall back to the 8.3 short path
# when the profile directory has one (quoting would break the PowerShell case).
$cmdPath = $exe.Replace("\", "/")
if ($cmdPath -match " ") {
    try {
        $short = (New-Object -ComObject Scripting.FileSystemObject).GetFile($exe).ShortPath
        if ($short -and $short -notmatch " ") { $cmdPath = $short.Replace("\", "/") }
    } catch {}
}

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
    command         = $cmdPath
    padding         = if ($null -ne $old.padding) { $old.padding } else { 0 }
    refreshInterval = if ($null -ne $old.refreshInterval) { $old.refreshInterval } else { 10 }
}
$settings | Add-Member -NotePropertyName statusLine -NotePropertyValue $statusLine -Force
$json = $settings | ConvertTo-Json -Depth 32
[IO.File]::WriteAllText($settingsPath, $json, (New-Object Text.UTF8Encoding $false))
Write-Host "statusLine configured in $settingsPath"
Write-Host "ccline installed to $exe"
