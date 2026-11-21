param(
    [switch]$DebugBuild,
    [switch]$ReleaseBuild
)
$ErrorActionPreference = 'Stop'
if ($DebugBuild -and $ReleaseBuild) { throw 'Choose either -DebugBuild or -ReleaseBuild' }
$profile = if ($DebugBuild) { 'dev' } elseif ($ReleaseBuild) { 'release' } else { 'fast' }
$outputProfile = if ($DebugBuild) { 'debug' } else { $profile }
if (Test-Path -LiteralPath 'D:\Tools\Rust\cargo\bin\cargo.exe') {
    $env:RUSTUP_HOME = 'D:\Tools\Rust\rustup'
    $env:CARGO_HOME = 'D:\Tools\Rust\cargo'
    $compiler = Get-ChildItem -LiteralPath 'D:\Tools' -Directory -Filter 'llvm-mingw-*-ucrt-x86_64' | Sort-Object Name -Descending | Select-Object -First 1
    if (!$compiler) { throw 'Portable LLVM-MinGW compiler not found in D:\Tools' }
    $env:PATH = 'D:\Tools\Rust\cargo\bin;' + (Join-Path $compiler.FullName 'bin') + ';' + $env:PATH
    $env:LIBRARY_PATH = Join-Path $env:RUSTUP_HOME 'toolchains\stable-x86_64-pc-windows-gnu\lib\rustlib\x86_64-pc-windows-gnu\lib\self-contained'
    $cargoArgs = @('+stable-x86_64-pc-windows-gnu','build','--locked','--target','x86_64-pc-windows-gnullvm')
    $outputBase = Join-Path $PSScriptRoot 'target\x86_64-pc-windows-gnullvm'
} else {
    $cargoArgs = @('build','--locked')
    $outputBase = Join-Path $PSScriptRoot 'target'
}
$cargoArgs += @('--profile',$profile,'--timings')
Push-Location -LiteralPath $PSScriptRoot
try {
    $buildTime = [System.Diagnostics.Stopwatch]::StartNew()
    Write-Host "Building profile: $profile"
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw 'Rust build failed' }
    $buildTime.Stop()
    Write-Host ('Build completed in {0:N1} seconds' -f $buildTime.Elapsed.TotalSeconds)
    $destination = Join-Path $PSScriptRoot 'Reword.exe'
    $running = Get-Process -Name Reword -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $destination }
    foreach ($process in $running) { Stop-Process -Id $process.Id; $process.WaitForExit() }
    Copy-Item -LiteralPath (Join-Path $outputBase "$outputProfile\reword.exe") -Destination $destination -Force
} finally { Pop-Location }
