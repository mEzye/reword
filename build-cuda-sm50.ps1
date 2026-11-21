param([ValidateRange(1,4)][int]$Jobs = 2)
$ErrorActionPreference = 'Stop'
$tools = 'D:\Tools'
$cudaRoot = Join-Path $tools 'CUDA-12.8'
$msvcRoot = Join-Path $tools 'PortableMSVC\msvc'
$work = Join-Path $PSScriptRoot 'target\cuda-sm50'
$source = Join-Path $work 'source\transcribe.cpp-0.4.2'
$build = Join-Path $work 'build'
$log = Join-Path $work 'build.log'
$cmake = 'C:\Program Files\CMake\bin\cmake.exe'
New-Item -ItemType Directory -Path $work -Force | Out-Null

function Invoke-BuildTool([string]$Program, [string[]]$Arguments) {
    $savedPreference = $ErrorActionPreference
    $writer = [System.IO.StreamWriter]::new($log, $true, [System.Text.UTF8Encoding]::new($false))
    $writer.AutoFlush = $true
    try {
        $ErrorActionPreference = 'Continue'
        & $Program @Arguments 2>&1 | ForEach-Object { $line=$_.ToString(); Write-Host $line; $writer.WriteLine($line) }
        $code = $LASTEXITCODE
    } finally { $writer.Dispose(); $ErrorActionPreference = $savedPreference }
    if ($code -ne 0) { throw "$Program exited with $code. See $log" }
}

if (!(Test-Path -LiteralPath (Join-Path $cudaRoot 'bin\nvcc.exe'))) { throw 'Portable CUDA 12.8 is not ready in D:\Tools\CUDA-12.8' }
$vc = Get-ChildItem -LiteralPath (Join-Path $msvcRoot 'VC\Tools\MSVC') -Directory | Sort-Object Name -Descending | Select-Object -First 1
$sdkRoot = Join-Path $msvcRoot 'Windows Kits\10'
$sdk = Get-ChildItem -LiteralPath (Join-Path $sdkRoot 'Include') -Directory | Sort-Object Name -Descending | Select-Object -First 1
if (!$vc -or !$sdk) { throw 'Portable MSVC/Windows SDK is not ready' }
$compilerBin = Join-Path $vc.FullName 'bin\Hostx64\x64'
$sdkBin = Join-Path $sdkRoot ('bin\' + $sdk.Name + '\x64')
$ninjaDir = Join-Path $tools 'Ninja'
if (!(Test-Path -LiteralPath (Join-Path $ninjaDir 'ninja.exe'))) {
    New-Item -ItemType Directory -Path $ninjaDir -Force | Out-Null
    $ninjaZip = Join-Path $tools 'Downloads\ninja-win-1.12.1.zip'
    & curl.exe -L --fail --retry 3 -o $ninjaZip 'https://github.com/ninja-build/ninja/releases/download/v1.12.1/ninja-win.zip'
    if ($LASTEXITCODE -ne 0) { throw 'Ninja download failed' }
    Expand-Archive -LiteralPath $ninjaZip -DestinationPath $ninjaDir -Force
}
if (!(Test-Path -LiteralPath (Join-Path $source 'CMakeLists.txt'))) {
    $archive = Join-Path $work 'source-v0.4.2.zip'
    & curl.exe -L --fail --retry 3 -o $archive 'https://codeload.github.com/handy-computer/transcribe.cpp/zip/refs/tags/v0.4.2'
    if ($LASTEXITCODE -ne 0) { throw 'transcribe.cpp source download failed' }
    Expand-Archive -LiteralPath $archive -DestinationPath (Join-Path $work 'source') -Force
}
$env:PATH = $compilerBin + ';' + $sdkBin + ';' + (Join-Path $cudaRoot 'bin') + ';' + $ninjaDir + ';D:\Tools\Python311;' + $env:PATH
$env:INCLUDE = (Join-Path $vc.FullName 'include') + ';' + (Join-Path $sdk.FullName 'ucrt') + ';' + (Join-Path $sdk.FullName 'shared') + ';' + (Join-Path $sdk.FullName 'um') + ';' + (Join-Path $sdk.FullName 'winrt')
$env:LIB = (Join-Path $vc.FullName 'lib\x64') + ';' + (Join-Path $sdkRoot ('Lib\' + $sdk.Name + '\ucrt\x64')) + ';' + (Join-Path $sdkRoot ('Lib\' + $sdk.Name + '\um\x64')) + ';' + (Join-Path $cudaRoot 'lib\x64')
$env:CUDA_PATH = $cudaRoot
$env:VCToolsInstallDir = $vc.FullName + '\'
$env:VSCMD_ARG_HOST_ARCH = 'x64'
$env:VSCMD_ARG_TGT_ARCH = 'x64'

if (!(Test-Path -LiteralPath (Join-Path $cudaRoot 'lib\x64\cuda.lib'))) {
    # Build only an import library from the installed driver's export names.
    # The NVIDIA driver itself is never copied or modified.
    $driver = Join-Path $env:WINDIR 'System32\nvcuda.dll'
    $exports = & 'D:\Tools\llvm-mingw-20261117-ucrt-x86_64\bin\llvm-readobj.exe' --coff-exports $driver
    $names = $exports | ForEach-Object { if ($_ -match '^\s*Name: (\w+)$') { $Matches[1] } }
    if (!$names) { throw 'Cannot read CUDA driver exports' }
    $definition = Join-Path $work 'cuda-driver.def'
    [System.IO.File]::WriteAllLines($definition, @('LIBRARY nvcuda.dll','EXPORTS') + $names)
    Invoke-BuildTool (Join-Path $compilerBin 'lib.exe') @('/nologo','/machine:x64',('/def:'+$definition),('/out:'+(Join-Path $cudaRoot 'lib\x64\cuda.lib')))
}

Write-Host "Building CUDA backend for sm_50 with MSVC $($vc.Name), CUDA 12.8, $Jobs jobs."
Write-Host "Progress log: $log"
Invoke-BuildTool $cmake @('-S',$source,'-B',$build,'-G','Ninja','-DCMAKE_BUILD_TYPE=Release',
    '-DCMAKE_C_COMPILER=cl.exe','-DCMAKE_CXX_COMPILER=cl.exe',
    ('-DCMAKE_CUDA_COMPILER='+(Join-Path $cudaRoot 'bin\nvcc.exe').Replace('\','/')),
    ('-DCMAKE_CUDA_HOST_COMPILER='+(Join-Path $compilerBin 'cl.exe').Replace('\','/')),
    ('-DCUDAToolkit_ROOT='+$cudaRoot.Replace('\','/')),'-DCMAKE_CUDA_ARCHITECTURES=50','-DCMAKE_CUDA_FLAGS=-Wno-deprecated-gpu-targets',
    '-DTRANSCRIBE_CUDA=ON','-DTRANSCRIBE_VULKAN=OFF','-DTRANSCRIBE_BUILD_SHARED=ON',
    '-DTRANSCRIBE_GGML_BACKEND_DL=ON','-DTRANSCRIBE_BUILD_TESTS=OFF','-DTRANSCRIBE_BUILD_EXAMPLES=OFF',
    '-DTRANSCRIBE_X86_CONSERVATIVE=ON','-DGGML_NATIVE=OFF','-DGGML_CCACHE=OFF','-DGGML_CUDA_FORCE_CUBLAS=ON','-DGGML_CUDA_FA=OFF','-DGGML_CUDA_GRAPHS=OFF')
Invoke-BuildTool $cmake @('--build',$build,'--config','Release','--target','ggml-cuda','--parallel',"$Jobs")
$artifact = Get-ChildItem -LiteralPath $build -Recurse -File -Filter 'ggml-cuda.dll' | Select-Object -First 1
if (!$artifact) { throw 'Build finished without ggml-cuda.dll' }
[System.IO.File]::WriteAllText((Join-Path $work 'artifact.txt'), $artifact.FullName)
Write-Host ('CUDA sm_50 backend built: ' + $artifact.FullName) -ForegroundColor Green
Write-Host 'The working runtime has not been replaced. Next step: test this backend on the 940MX.'
