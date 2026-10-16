param([Parameter(Mandatory=$true)][string]$Executable,[Parameter(Mandatory=$true)][string]$OutputDirectory,[string]$Distro='Ubuntu')
& (Join-Path $PSScriptRoot 'test-windows-workbench.ps1') -Executable $Executable -OutputDirectory $OutputDirectory -Distro $Distro -Mode Terminal
