param([switch]$DebugBuild)
$ErrorActionPreference = 'Stop'
if (Test-Path -LiteralPath 'D:\Tools\Rust\cargo\bin\cargo.exe') {
    $env:RUSTUP_HOME = 'D:\Tools\Rust\rustup'
    $env:CARGO_HOME = 'D:\Tools\Rust\cargo'
    $compiler = Get-ChildItem -LiteralPath 'D:\Tools' -Directory -Filter 'llvm-mingw-*-ucrt-x86_64' | Sort-Object Name -Descending | Select-Object -First 1
    if (!$compiler) { throw 'Portable LLVM-MinGW compiler not found in D:\Tools' }
    $env:PATH = 'D:\Tools\Rust\cargo\bin;' + (Join-Path $compiler.FullName 'bin') + ';' + $env:PATH
    $env:LIBRARY_PATH = Join-Path $env:RUSTUP_HOME 'toolchains\stable-x86_64-pc-windows-gnu\lib\rustlib\x86_64-pc-windows-gnu\lib\self-contained'
    $cargoAction = if ($DebugBuild) { 'build' } else { 'rustc' }
    $cargoArgs = @('+stable-x86_64-pc-windows-gnu',$cargoAction,'--locked','--target','x86_64-pc-windows-gnullvm')
    $outputBase = Join-Path $PSScriptRoot 'target\x86_64-pc-windows-gnullvm'
} else {
    $cargoAction = if ($DebugBuild) { 'build' } else { 'rustc' }
    $cargoArgs = @($cargoAction,'--locked')
    $outputBase = Join-Path $PSScriptRoot 'target'
}
if (!$DebugBuild) { $cargoArgs += @('--release','--','-C','opt-level=1') }
Push-Location -LiteralPath $PSScriptRoot
try {
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw 'Rust build failed' }
    $profile = if ($DebugBuild) { 'debug' } else { 'release' }
    $destination = Join-Path $PSScriptRoot 'Reword.exe'
    $running = Get-Process -Name Reword -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $destination }
    foreach ($process in $running) { Stop-Process -Id $process.Id; $process.WaitForExit() }
    Copy-Item -LiteralPath (Join-Path $outputBase "$profile\reword.exe") -Destination $destination -Force
} finally { Pop-Location }
