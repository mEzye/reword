param(
    [string]$Listen = '0.0.0.0:8178',
    [ValidateSet('cpu','cuda','auto')][string]$Device = 'cpu',
    [ValidateRange(1,32)][int]$Threads = 2
)
$ErrorActionPreference = 'Stop'
$data = if ($env:REWORD_DATA_DIR) { $env:REWORD_DATA_DIR } else { Join-Path $env:LOCALAPPDATA 'Reword' }
$log = Join-Path $data 'logs\speech.log'
$offset = if (Test-Path -LiteralPath $log) { (Get-Item -LiteralPath $log).Length } else { 0 }
$server = Start-Process -FilePath (Join-Path $PSScriptRoot 'Reword.exe') -WorkingDirectory $PSScriptRoot -WindowStyle Hidden -PassThru -ArgumentList @('--speech-server',"--listen=$Listen","--device=$Device","--threads=$Threads")
$null = $server.Handle
try {
    Write-Host "Reword speech server. Ctrl+C stops this server. Log: $log"
    do {
        if (Test-Path -LiteralPath $log) {
            $file = $null
            $reader = $null
            try {
                $file = [System.IO.File]::Open($log, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
                if ($file.Length -lt $offset) { $offset = 0 }
                [void]$file.Seek($offset, [System.IO.SeekOrigin]::Begin)
                $reader = [System.IO.StreamReader]::new($file, [System.Text.Encoding]::UTF8)
                $text = $reader.ReadToEnd()
                $offset = $file.Position
                if ($text) { Write-Host -NoNewline $text }
            } catch [System.IO.IOException] {
                # A writer may briefly hold the log lock; retry on the next poll.
            } finally {
                if ($reader) { $reader.Dispose() }
                if ($file) { $file.Dispose() }
            }
        }
    } while (!$server.WaitForExit(500))
    if ($server.ExitCode -ne 0) { throw "Speech server exited with code $($server.ExitCode). Log: $log" }
} finally {
    if (!$server.HasExited) { Stop-Process -Id $server.Id; $server.WaitForExit() }
}
