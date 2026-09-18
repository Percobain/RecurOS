# Install the `ctx` binary (ContextOS) on Windows.
#
#   irm https://raw.githubusercontent.com/Percobain/ContextOS/main/install.ps1 | iex
#
# Installs to %LOCALAPPDATA%\Programs\ctx and adds it to your user PATH.
# No Docker, Node, Python or admin rights needed.
$ErrorActionPreference = 'Stop'

$Repo = if ($env:CTX_REPO) { $env:CTX_REPO } else { 'Percobain/ContextOS' }
$Dir = if ($env:CTX_INSTALL_DIR) { $env:CTX_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\ctx' }
$Url = "https://github.com/$Repo/releases/latest/download/ctx-x86_64-pc-windows-msvc.zip"

$Tmp = Join-Path ([IO.Path]::GetTempPath()) ("ctx-" + [guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $Tmp | Out-Null
try {
    Write-Host "downloading $Url"
    Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile (Join-Path $Tmp 'ctx.zip')
    Expand-Archive -Force -Path (Join-Path $Tmp 'ctx.zip') -DestinationPath $Tmp
    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    Copy-Item -Force (Join-Path $Tmp 'ctx.exe') (Join-Path $Dir 'ctx.exe')
} finally {
    Remove-Item -Recurse -Force $Tmp -ErrorAction SilentlyContinue
}

$UserPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not ($UserPath -split ';' | Where-Object { $_ -eq $Dir })) {
    [Environment]::SetEnvironmentVariable('Path', "$UserPath;$Dir", 'User')
    Write-Host "added $Dir to your user PATH (open a new terminal to use it)"
}
& (Join-Path $Dir 'ctx.exe') --version
Write-Host ""
Write-Host "next: cd into a project and run  ctx init"
