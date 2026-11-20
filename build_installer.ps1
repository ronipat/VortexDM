# Build VortexDM (Release) + Windows Installer (.exe)
$ErrorActionPreference = "Stop"

$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $Root

Write-Host "==> Building VortexDM release binary with Cargo..." -ForegroundColor Cyan
cargo build --release
if ($LASTEXITCODE -ne 0) {
    throw "Cargo release build failed with exit code $LASTEXITCODE"
}

$ExePath = Join-Path $Root "target\release\vortexdm.exe"
if (-not (Test-Path $ExePath)) {
    throw "Release binary not found at $ExePath"
}

$IsccCandidates = @(
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles}\Inno Setup 6\ISCC.exe"
)

$Iscc = $IsccCandidates | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $Iscc) {
    throw "Inno Setup 6 (ISCC.exe) not found. Install via: winget install JRSoftware.InnoSetup --scope user"
}

$DistDir = Join-Path $Root "dist"
if (Test-Path $DistDir) {
    Get-ChildItem $DistDir -Filter "VortexDM-Setup-*.exe" | Remove-Item -Force -ErrorAction SilentlyContinue
}

Write-Host "==> Compiling Windows Installer with Inno Setup 6..." -ForegroundColor Cyan
& $Iscc (Join-Path $Root "installer\VortexDM.iss")
if ($LASTEXITCODE -ne 0) {
    throw "Inno Setup compiler failed with exit code $LASTEXITCODE"
}

$SetupExe = Join-Path $Root "dist\VortexDM-Setup-1.1.0.exe"
if (Test-Path $SetupExe) {
    $SizeMB = [Math]::Round((Get-Item $SetupExe).Length / 1MB, 2)
    Write-Host "==> Installer created: $SetupExe ($SizeMB MB)" -ForegroundColor Green

    # Notify Windows Explorer shell to refresh icon cache immediately
    Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class ShellRefresh {
    [DllImport("shell32.dll")]
    public static extern void SHChangeNotify(int wEventId, uint uFlags, IntPtr dwItem1, IntPtr dwItem2);
}
"@ -ErrorAction SilentlyContinue
    try { [ShellRefresh]::SHChangeNotify(0x08000000, 0x0000, [IntPtr]::Zero, [IntPtr]::Zero) } catch {}
    $ie4 = Join-Path $env:SystemRoot "System32\ie4uinit.exe"
    if (Test-Path $ie4) {
        Start-Process -FilePath $ie4 -ArgumentList "-show" -NoNewWindow -Wait -ErrorAction SilentlyContinue
    }
}
