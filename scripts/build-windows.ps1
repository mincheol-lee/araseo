$ErrorActionPreference = "Stop"
$env:CARGO_INCREMENTAL = "0"

$RepositoryRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepositoryRoot

cargo test --locked --manifest-path (Join-Path $RepositoryRoot "harness\Cargo.toml")
if ($LASTEXITCODE -ne 0) { throw "Harness tests failed." }
cargo build --locked --release
if ($LASTEXITCODE -ne 0) { throw "Windows release build failed." }

$DistDirectory = Join-Path $RepositoryRoot "dist"
New-Item -ItemType Directory -Force -Path $DistDirectory | Out-Null
$LiveExecutable = Join-Path $DistDirectory 'araseo.exe'
$LiveProcess = Get-Process araseo -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $LiveExecutable }
$OutputName = if ($LiveProcess) { 'araseo-next' } else { 'araseo' }
$OutputExecutable = Join-Path $DistDirectory "$OutputName.exe"
if (Get-Process araseo-next -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $OutputExecutable }) {
    throw 'The staged executable is running. Close it before replacing this build.'
}
Copy-Item -Force (Join-Path $RepositoryRoot "target\release\araseo.exe") $OutputExecutable

$Symbols = Join-Path $RepositoryRoot "target\release\araseo.pdb"
if (Test-Path $Symbols) { Copy-Item -Force $Symbols (Join-Path $DistDirectory "$OutputName.pdb") }
Copy-Item -Force (Join-Path $RepositoryRoot 'THIRD-PARTY-NOTICES.txt') (Join-Path $DistDirectory 'THIRD-PARTY-NOTICES.txt')
$Revision = (git rev-parse HEAD).Trim()
$Hash = (Get-FileHash -Algorithm SHA256 $OutputExecutable).Hash
[System.IO.File]::WriteAllText((Join-Path $DistDirectory 'build-info.txt'), "revision=$Revision`nsha256=$Hash`n", [System.Text.Encoding]::ASCII)

Write-Host "Built: $OutputExecutable"
if ($LiveProcess) { Write-Host 'araseo.exe is running; the new build is staged as araseo-next.exe.' }
Write-Host "Set ARASEO_EXE in WSL to the /mnt/c/... path of this executable."
