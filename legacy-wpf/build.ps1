$ErrorActionPreference = 'Stop'
$framework = Join-Path $env:WINDIR 'Microsoft.NET\Framework64\v4.0.30319'
$references = @('System.dll','System.Core.dll','System.Drawing.dll','System.Windows.Forms.dll','WPF\WindowsBase.dll','WPF\PresentationCore.dll','WPF\PresentationFramework.dll','System.Xaml.dll')
$compilerArgs = @('/nologo','/target:winexe','/platform:x64','/optimize+',('/out:' + (Join-Path $PSScriptRoot 'Reword.exe')))
$iconPath = Join-Path $PSScriptRoot 'assets\Reword.ico'
if (Test-Path -LiteralPath $iconPath) { $compilerArgs += '/win32icon:' + $iconPath }
foreach ($reference in $references) { $compilerArgs += '/reference:' + (Join-Path $framework $reference) }
$compilerArgs += Join-Path $PSScriptRoot 'src\Reword.cs'
& (Join-Path $framework 'csc.exe') @compilerArgs
if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
