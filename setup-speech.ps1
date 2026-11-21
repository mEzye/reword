param([ValidateSet('cpu','cuda')][string]$Backend = 'cpu')
$ErrorActionPreference = 'Stop'
$modelDir = Join-Path $PSScriptRoot 'models'
$runtimeDir = Join-Path $PSScriptRoot 'speech-runtime'
New-Item -ItemType Directory -Path $modelDir,$runtimeDir -Force | Out-Null
function Download-Verified([string]$Url, [string]$Destination, [string]$Sha256) {
    if ((Test-Path -LiteralPath $Destination) -and (Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash -eq $Sha256) { return }
    $partial = $Destination + '.partial'
    & curl.exe -L --fail --retry 3 --continue-at - --output $partial $Url
    if ($LASTEXITCODE -ne 0) { throw "Download failed: $Url. Run this script again to resume." }
    if ((Get-FileHash -LiteralPath $partial -Algorithm SHA256).Hash -ne $Sha256) {
        Remove-Item -LiteralPath $partial -Force
        throw "SHA-256 mismatch; run setup again: $Destination"
    }
    Move-Item -LiteralPath $partial -Destination $Destination -Force
}
$variant = if ($Backend -eq 'cuda') { 'cuda' } else { 'cpu-vulkan' }
$archive = Join-Path $runtimeDir "transcribe-0.4.2-$variant.tar.gz"
$digest = if ($Backend -eq 'cuda') { '7c2410adb70cfa22458322e483b41a4588edb5ad9f1d718eddd5992bbded8124' } else { '6094102cf57b404b050707ceb258b12ce647b92e801b5d46077214e5b030c8aa' }
Write-Host "Downloading native speech runtime ($Backend)..."
Download-Verified "https://github.com/handy-computer/transcribe.cpp/releases/download/v0.4.2/transcribe-native-0.4.2-windows-x86_64-$variant.tar.gz" $archive $digest
$nativeDir = Join-Path $runtimeDir "transcribe-native-windows-x86_64-$variant"
if (!(Test-Path -LiteralPath (Join-Path $nativeDir 'transcribe.dll'))) {
    & tar -xf $archive -C $runtimeDir
    if ($LASTEXITCODE -ne 0) { throw 'Runtime extraction failed' }
}
if ($Backend -eq 'cuda') {
    # NVIDIA's official redistributable libraries, kept private to this app.
    # No system-wide CUDA SDK installation or PATH changes are required.
    $components = @(
        @{Path='cuda_cudart/windows-x86_64/cuda_cudart-windows-x86_64-12.8.90-archive.zip';Hash='4a39058fd8519444a81cfc7ae055d136f48d1a31ffa41ae255b35b2edd61e13b';Dll='cudart64_12.dll'},
        @{Path='libcublas/windows-x86_64/libcublas-windows-x86_64-12.8.4.1-archive.zip';Hash='57a470112cec7e112c95253dde8b3c7184d795dbd92b0bde77a4cb7f8c94c8aa';Dll='cublas64_12.dll'}
    )
    foreach ($component in $components) {
        if (Test-Path -LiteralPath (Join-Path $nativeDir $component.Dll)) { continue }
        $componentZip = Join-Path $runtimeDir ([System.IO.Path]::GetFileName($component.Path))
        Write-Host "Downloading NVIDIA component $($component.Dll)..."
        Download-Verified ('https://developer.download.nvidia.com/compute/cuda/redist/' + $component.Path) $componentZip $component.Hash
        & tar -xf $componentZip -C $runtimeDir
        if ($LASTEXITCODE -ne 0) { throw 'NVIDIA runtime extraction failed' }
        $componentDir = Join-Path $runtimeDir ([System.IO.Path]::GetFileNameWithoutExtension($componentZip))
        Get-ChildItem -LiteralPath $componentDir -Recurse -Filter '*.dll' | ForEach-Object {
            Copy-Item -LiteralPath $_.FullName -Destination $nativeDir -Force
        }
    }
}
Write-Host 'Downloading Ukrainian Parakeet v3 Q8 model (about 740 MB)...'
Download-Verified 'https://huggingface.co/vladkens/parakeet-tdt-0.6b-v3-uk-gguf/resolve/df469cd0d1944c4368bef64a37c7f0393d00b42c/parakeet-tdt-0.6b-v3-uk-Q8_0.gguf' (Join-Path $modelDir 'parakeet-uk-v3-q8.gguf') '7645a16ed64f5ceb12e6b4a3cd82558dbbc7e1d505842d08a534ecfdfa471d2d'
Write-Host 'Speech files are ready.' -ForegroundColor Green
