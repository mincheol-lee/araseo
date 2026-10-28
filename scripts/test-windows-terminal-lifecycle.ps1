param([Parameter(Mandatory=$true)][string]$ProbeExecutable,[string]$Distro='Ubuntu')
$ErrorActionPreference='Stop'
$root='/tmp/araseo-lifecycle-native-'+[Guid]::NewGuid().ToString('N')
$drivers=New-Object Collections.Generic.List[Diagnostics.Process]
$legacy=New-Object Collections.Generic.List[object]
function Python([string]$code,[string]$argument=$root){
 $encoded=[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($code))
 $output=& wsl.exe -d $Distro --exec python3 -c "exec(__import__('base64').b64decode('$encoded'))" $argument
 if($LASTEXITCODE-ne 0){throw 'Private fixture operation failed'}
 return $output
}
function Line($process){$read=$process.StandardOutput.ReadLineAsync();if(!$read.Wait(20000)){throw 'Driver response timed out'};return $read.Result}
function Send($process,[string]$command){$process.StandardInput.WriteLine($command);$process.StandardInput.Flush();return Line $process}
function Start-Driver {
 $info=New-Object Diagnostics.ProcessStartInfo
 $info.FileName=[IO.Path]::GetFullPath($ProbeExecutable);$info.Arguments='"'+$Distro+'" "'+$root+'"'
 $info.UseShellExecute=$false;$info.CreateNoWindow=$true
 $info.RedirectStandardInput=$true;$info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true
 $process=[Diagnostics.Process]::Start($info);$drivers.Add($process)
 $line=Line $process
 if($line-ne 'READY'){throw "Driver failed: $line"}
 return $process
}
function Read-Case([string]$name){
 $deadline=(Get-Date).AddSeconds(12)
 do{$result=Python ('import pathlib,sys; p=pathlib.Path(sys.argv[1])/"'+$name+'.json"; print(p.read_text() if p.exists() else "")');if($result){return $result|ConvertFrom-Json};Start-Sleep -Milliseconds 100}while((Get-Date)-lt $deadline)
 throw "Fixture $name did not start"
}
function Start-Case($process,[string]$name){
 $reply=Send $process "input python3 $root/fixture.py $root $name"
 if($reply-ne 'SENT'){throw "Failed input: $reply"}
 return Read-Case $name
}
function Assert-Closed([string]$name){
 $code='import json,pathlib,sys; r=pathlib.Path(sys.argv[1]); d=json.loads((r/"'+$name+'.json").read_text()); active=lambda p: pathlib.Path("/proc/"+str(p)+"/stat").exists() and pathlib.Path("/proc/"+str(p)+"/stat").read_text().rsplit(") ",1)[1].split()[0]!="Z"; print(not active(d["pid"]) and not active(d["child"]) and (r/"'+$name+'.hup").exists())'
 $deadline=(Get-Date).AddSeconds(10)
 do{if((Python $code)-eq 'True'){return};Start-Sleep -Milliseconds 150}while((Get-Date)-lt $deadline)
 throw "Owned process tree survived: $name"
}
function Assert-Alive([string]$name){
 Python ('import json,pathlib,sys; r=pathlib.Path(sys.argv[1]); d=json.loads((r/"'+$name+'.json").read_text()); assert pathlib.Path("/proc/"+str(d["pid"])).exists(); assert not (r/"'+$name+'.hup").exists()')|Out-Null
}
function Kill-Process($process){$process.Refresh();if(!$process.HasExited){$process.Kill();$process.WaitForExit()}}
function Start-Legacy([uint32]$owner,[uint32]$id,[string]$name){
 $tty="/tmp/araseo-pty-$owner-$id"
 $command="stty rows 24 cols 80; tty > $tty; python3 $root/fixture.py $root $name"
 $info=New-Object Diagnostics.ProcessStartInfo
 $info.FileName='C:\Windows\System32\wsl.exe';$info.Arguments='-d '+$Distro+' --exec /usr/bin/script -qfec "'+$command+'" /dev/null'
 $info.UseShellExecute=$false;$info.CreateNoWindow=$true
 $info.RedirectStandardInput=$true;$info.RedirectStandardOutput=$true;$info.RedirectStandardError=$true
 $process=[Diagnostics.Process]::Start($info);$drivers.Add($process)
 $null=Read-Case $name
 Kill-Process $process # Reproduce the original orphaning bug.
 $legacy.Add([pscustomobject]@{owner=$owner;id=$id})
}
try {
 $fixture=[IO.File]::ReadAllText((Join-Path $PSScriptRoot '../ui-harness/fixtures/terminal-lifecycle.py'))
 $encoded=[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($fixture))
 Python "import pathlib,sys,base64; r=pathlib.Path(sys.argv[1]); r.mkdir(); (r/'fixture.py').write_bytes(base64.b64decode('$encoded'))"|Out-Null
 $isAdmin=([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
 Write-Output "Native lifecycle checks; administrator=$isAdmin"
 $first=Start-Driver;$peer=Start-Driver
 $closed=Start-Case $first 'closed';$null=Start-Case $peer 'peer'
 # Freeze only this fixture's script bridge, then fill its data pipe. Closing
 # must use the independent stop event even while a terminal write is blocked.
 $pause=@'
import json,os,pathlib,signal,sys
r=pathlib.Path(sys.argv[1]); token=json.loads((r/'closed.json').read_text())['token']
for entry in pathlib.Path('/proc').iterdir():
 if not entry.name.isdigit(): continue
 try:
  if os.path.basename(os.readlink(entry/'exe'))!='script': continue
  if ('ARASEO_TERMINAL_SESSION='+token).encode() not in (entry/'environ').read_bytes().split(b'\0'): continue
  fd=os.pidfd_open(int(entry.name))
  try: signal.pidfd_send_signal(fd,signal.SIGSTOP)
  finally: os.close(fd)
 except (OSError,ValueError): continue
'@
 Python $pause|Out-Null
 if((Send $first ('input '+('x'*2097152)))-ne 'SENT'){throw 'Blocked-input fixture failed'}
 $watch=[Diagnostics.Stopwatch]::StartNew();$reply=Send $first 'drop';$watch.Stop()
 if($reply-ne 'DROPPED' -or $watch.ElapsedMilliseconds-gt 500){throw 'Closing a terminal blocked its caller'}
 Assert-Closed 'closed';Assert-Alive 'peer'
 Write-Output 'Terminal close: graceful HUP, stubborn detached child removed, peer preserved, blocked input did not delay caller: passed'
 Kill-Process $peer
 Assert-Closed 'peer'
 Write-Output 'IDE owner force-kill without Drop: full Linux tree removed: passed'
 $proxy=Start-Driver;$null=Start-Case $proxy 'proxy'
 if((Send $proxy 'resize')-ne 'RESIZED'){throw 'Resize dispatch failed'}
 Python 'import json,pathlib,sys,fcntl,termios,struct; r=pathlib.Path(sys.argv[1]); d=json.loads((r/"proxy.json").read_text()); f=open(d["tty"],"rb",buffering=0); assert struct.unpack("HHHH",fcntl.ioctl(f,termios.TIOCGWINSZ,b"\0"*8))[:2]==(40,120)'|Out-Null
 $terminalHost=Get-CimInstance Win32_Process|Where-Object {$_.ParentProcessId-eq $proxy.Id -and $_.CommandLine-like '*--araseo-terminal-host*'}|Select-Object -First 1
 if(!$terminalHost){throw 'Missing private terminal host'}
 $bridge=Get-CimInstance Win32_Process|Where-Object {$_.ParentProcessId-eq $terminalHost.ProcessId -and $_.Name-eq 'wsl.exe'}|Select-Object -First 1
 if(!$bridge){throw 'Missing WSL transport'}
 Stop-Process -Id $bridge.ProcessId -Force
 Assert-Closed 'proxy'
 Write-Output 'Direct WSL proxy termination and existing PTY resize: passed'
 # A private-host crash still hangs up the foreground CLI. Its stubborn orphan
 # is recovered by the next owner after this owner also exits.
 $crash=Start-Driver;$null=Start-Case $crash 'host-crash'
 $terminalHost=Get-CimInstance Win32_Process|Where-Object {$_.ParentProcessId-eq $crash.Id -and $_.CommandLine-like '*--araseo-terminal-host*'}|Select-Object -First 1
 Stop-Process -Id $terminalHost.ProcessId -Force
 $deadline=(Get-Date).AddSeconds(8)
 do{$hungup=Python 'import pathlib,sys; print((pathlib.Path(sys.argv[1])/"host-crash.hup").exists())';if($hungup-eq 'True'){break};Start-Sleep -Milliseconds 100}while((Get-Date)-lt $deadline)
 if($hungup-ne 'True'){throw 'PTY hangup backstop failed after host crash'}
 Kill-Process $crash
 Start-Legacy $peer.Id 2147483646 'legacy-dead'
 Start-Legacy $PID 2147483645 'legacy-live'
 Assert-Alive 'legacy-dead';Assert-Alive 'legacy-live'
 $restart=Start-Driver
 Assert-Closed 'host-crash';Assert-Closed 'legacy-dead';Assert-Alive 'legacy-live'
 Write-Output 'Restart recovery: marked orphan and old script session removed; live legacy owner preserved: passed'
 $earlyTokens=@()
 foreach($index in 1..8){
  $early=Start-Driver
  $earlyTokens+=([string]$early.Id+'.'+[string]$early.StartTime.ToFileTimeUtc()+'.1')
  Kill-Process $early
 }
 Start-Sleep -Milliseconds 1500
 $cleanup=[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes([IO.File]::ReadAllText((Join-Path $PSScriptRoot '../src/terminal_cleanup.py'))))
 $owned=(Python "import sys,base64; sys.argv=['cleanup','list']; exec(base64.b64decode('$cleanup'))")|ConvertFrom-Json
 foreach($item in $owned){if($earlyTokens-contains $item.token){throw 'Early owner exit left a session behind'}}
 Write-Output 'Eight owner-exit/startup races left no marked terminal processes: passed'
 Write-Output 'Native Windows terminal lifecycle verification passed.'
} finally {
 foreach($driver in $drivers){try{Kill-Process $driver}catch{}}
 # Release only private fixtures created by this test, including the deliberately
 # unmanaged legacy session whose fake Windows owner was kept alive above.
 Python 'import json,os,pathlib,signal,sys; r=pathlib.Path(sys.argv[1]); assert r.name.startswith("araseo-lifecycle-native-");
for p in r.glob("*.json"):
 d=json.loads(p.read_text())
 for key in ("pid","child"):
  try:
   fd=os.pidfd_open(d[key])
   try:
    start=int(pathlib.Path("/proc/"+str(d[key])+"/stat").read_text().rsplit(") ",1)[1].split()[19])
    if start==d["start" if key=="pid" else "child_start"]: signal.pidfd_send_signal(fd,signal.SIGKILL)
   finally: os.close(fd)
  except (ProcessLookupError,FileNotFoundError): pass'|Out-Null
 foreach($entry in $legacy){
  $path="/tmp/araseo-pty-$($entry.owner)-$($entry.id)"
  Python ("import pathlib; pathlib.Path('$path').unlink(missing_ok=True)")|Out-Null
 }
 Python 'import shutil,sys; p=sys.argv[1]; assert p.startswith("/tmp/araseo-lifecycle-native-"); shutil.rmtree(p)'|Out-Null
}
