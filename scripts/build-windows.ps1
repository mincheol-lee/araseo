$ErrorActionPreference = "Stop"

$RepositoryRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepositoryRoot

cargo test --locked --manifest-path (Join-Path $RepositoryRoot "harness\Cargo.toml")
if ($LASTEXITCODE -ne 0) { throw "Harness tests failed." }
cargo build --locked --release
if ($LASTEXITCODE -ne 0) { throw "Windows release build failed." }

$DistDirectory = Join-Path $RepositoryRoot "dist"
New-Item -ItemType Directory -Force -Path $DistDirectory | Out-Null
Copy-Item -Force (Join-Path $RepositoryRoot "target\release\araseo.exe") (Join-Path $DistDirectory "araseo.exe")

$Symbols = Join-Path $RepositoryRoot "target\release\araseo.pdb"
if (Test-Path $Symbols) { Copy-Item -Force $Symbols (Join-Path $DistDirectory "araseo.pdb") }
$Revision = (git rev-parse HEAD).Trim()
$Hash = (Get-FileHash -Algorithm SHA256 (Join-Path $DistDirectory "araseo.exe")).Hash
[System.IO.File]::WriteAllText((Join-Path $DistDirectory 'build-info.txt'), "revision=$Revision`nsha256=$Hash`n", [System.Text.Encoding]::ASCII)

Write-Host "Built: $DistDirectory\araseo.exe"
Write-Host "Set ARASEO_EXE in WSL to the /mnt/c/... path of this executable."
