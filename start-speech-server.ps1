param(
    [string]$Listen = '0.0.0.0:8178',
    [ValidateSet('cpu','cuda','auto')][string]$Device = 'cpu',
    [ValidateRange(1,32)][int]$Threads = 2
)
$ErrorActionPreference = 'Stop'
$serverLog = Join-Path $PSScriptRoot 'speech-server-console.log'
$server = Start-Process -FilePath (Join-Path $PSScriptRoot 'Reword.exe') -WorkingDirectory $PSScriptRoot -WindowStyle Hidden -PassThru -RedirectStandardError $serverLog -ArgumentList @('--speech-server',"--listen=$Listen","--device=$Device","--threads=$Threads")
$null = $server.Handle
$reader = $null
try {
    $file = [System.IO.File]::Open($serverLog, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
    $reader = [System.IO.StreamReader]::new($file)
    Write-Host 'Reword speech server. Ctrl+C stops this server.'
    do {
        $text = $reader.ReadToEnd()
        if ($text) { Write-Host -NoNewline $text }
    } while (!$server.WaitForExit(500))
    $text = $reader.ReadToEnd()
    if ($text) { Write-Host -NoNewline $text }
    if ($server.ExitCode -ne 0) { throw "Speech server exited with code $($server.ExitCode)" }
} finally {
    if ($reader) { $reader.Dispose() }
    if (!$server.HasExited) { Stop-Process -Id $server.Id; $server.WaitForExit() }
}
