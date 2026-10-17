param(
 [Parameter(Mandatory=$true)][string]$Executable,
 [Parameter(Mandatory=$true)][string]$OutputDirectory,
 [string]$Distro='Ubuntu',
 [ValidateSet('All','Terminal','TerminalInput','Resize','Git','Agent')][string]$Mode='All'
)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type @'
using System;using System.Runtime.InteropServices;
public static class WorkbenchInput {
 [StructLayout(LayoutKind.Sequential)] public struct Point{public int x,y;}
 [StructLayout(LayoutKind.Sequential)] public struct Rect{public int l,t,r,b;}
 [StructLayout(LayoutKind.Sequential)] public struct Keyboard{public ushort vk,scan;public uint flags,time;public UIntPtr extra;}
 [StructLayout(LayoutKind.Explicit,Size=40)]public struct Input{[FieldOffset(0)]public uint type;[FieldOffset(8)]public Keyboard key;}
 [DllImport("user32.dll")]public static extern bool SetProcessDPIAware();
 [DllImport("user32.dll")]public static extern bool SetForegroundWindow(IntPtr h);
 [DllImport("user32.dll")]public static extern IntPtr GetForegroundWindow();
 [DllImport("user32.dll")]public static extern uint GetDpiForWindow(IntPtr h);
 [DllImport("user32.dll",EntryPoint="GetClassLongPtrW")]public static extern IntPtr GetClassLongPtr(IntPtr h,int index);
 [DllImport("user32.dll")]public static extern bool GetClientRect(IntPtr h,out Rect r);
 [DllImport("user32.dll")]public static extern bool ClientToScreen(IntPtr h,ref Point p);
 [DllImport("user32.dll")]public static extern bool MoveWindow(IntPtr h,int x,int y,int w,int hgt,bool repaint);
 [DllImport("user32.dll")]public static extern bool ShowWindow(IntPtr h,int command);
 [DllImport("user32.dll")]public static extern bool IsZoomed(IntPtr h);
 [DllImport("user32.dll")]public static extern bool IsIconic(IntPtr h);
 [DllImport("user32.dll")]public static extern bool InvalidateRect(IntPtr h,IntPtr r,bool erase);
 [DllImport("user32.dll")]public static extern bool SetCursorPos(int x,int y);
 [DllImport("user32.dll")]public static extern void mouse_event(uint flags,uint x,uint y,uint data,UIntPtr extra);
 [DllImport("user32.dll")]static extern uint SendInput(uint count,Input[] input,int size);
 public static void Text(string text){foreach(char c in text){var a=new Input[2];a[0].type=1;a[0].key.scan=c;a[0].key.flags=4;a[1]=a[0];a[1].key.flags=6;if(SendInput(2,a,40)!=2)throw new Exception("SendInput failed");}}
 public static void Enter(){var a=new Input[2];a[0].type=1;a[0].key.vk=13;a[1]=a[0];a[1].key.flags=2;if(SendInput(2,a,40)!=2)throw new Exception("Enter failed");}
 public static void Tab(bool backwards){var a=new Input[backwards?4:2];int i=0;if(backwards){a[i].type=1;a[i++].key.vk=16;}a[i].type=1;a[i++].key.vk=9;a[i].type=1;a[i].key.vk=9;a[i++].key.flags=2;if(backwards){a[i].type=1;a[i].key.vk=16;a[i].key.flags=2;}if(SendInput((uint)a.Length,a,40)!=a.Length)throw new Exception("Tab failed");}
 public static void Chord(ushort vk,int modifiers){var keys=new System.Collections.Generic.List<ushort>();if((modifiers&1)!=0)keys.Add(16);if((modifiers&2)!=0)keys.Add(18);if((modifiers&4)!=0)keys.Add(17);var a=new Input[2+2*keys.Count];int i=0;foreach(var k in keys){a[i].type=1;a[i++].key.vk=k;}a[i].type=1;a[i++].key.vk=vk;a[i].type=1;a[i].key.vk=vk;a[i++].key.flags=2;keys.Reverse();foreach(var k in keys){a[i].type=1;a[i].key.vk=k;a[i++].key.flags=2;}if(SendInput((uint)a.Length,a,40)!=a.Length)throw new Exception("Chord failed");}
 public static void Wheel(int delta){mouse_event(0x0800,0,0,unchecked((uint)delta),UIntPtr.Zero);}
}
'@
[WorkbenchInput]::SetProcessDPIAware()|Out-Null
$OutputDirectory=[IO.Path]::GetFullPath($OutputDirectory)
New-Item -ItemType Directory -Force $OutputDirectory|Out-Null
$workspace='/tmp/araseo-gpui-check-'+[Guid]::NewGuid().ToString('N')
function Invoke-PrivatePython([string]$Code,[string]$Path){
 $encoded=[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($Code))
 & wsl.exe -d $Distro --exec /usr/bin/python3 -c "exec(__import__('base64').b64decode('$encoded'))" $Path
 if($LASTEXITCODE-ne 0){throw 'Private WSL fixture operation failed'}
}
Invoke-PrivatePython 'import pathlib,sys; p=pathlib.Path(sys.argv[1]); p.mkdir(); (p/"README.md").write_text("# GPUI native validation\n",encoding="utf-8")' $workspace
if($Mode -eq 'TerminalInput'){
 $fixture=[IO.File]::ReadAllText((Join-Path $PSScriptRoot '../ui-harness/fixtures/terminal-input.py'))
 $encodedFixture=[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($fixture))
 Invoke-PrivatePython "import base64,pathlib,sys; (pathlib.Path(sys.argv[1])/'terminal-input.py').write_bytes(base64.b64decode('$encodedFixture'))" $workspace
}
if($Mode -eq 'Agent'){
 $agentFixture=@'
import pathlib,sys
root=pathlib.Path(sys.argv[1])
(root/'control').write_text('waiting')
(root/'agent.py').write_text('''import ctypes,os,pathlib,sys,time
name=sys.argv[1]
ctypes.CDLL(None).prctl(15,name.encode(),0,0,0)
control=pathlib.Path(__file__).with_name('control')
status=pathlib.Path(os.environ['ARASEO_AGENT_STATE_FILE'])
previous=None
while True:
    value=control.read_text().strip()
    if value=='exit':break
    if value!=previous:
        status.write_text(name+':'+value)
        previous=value
    time.sleep(0.05)
''')
'@
 Invoke-PrivatePython $agentFixture $workspace
}
if($Mode -eq 'Git'){
 $gitFixture=@'
import pathlib,sys,subprocess
root=pathlib.Path(sys.argv[1])
for name in ('alpha','beta'):
    repo=root/name
    (repo/'src').mkdir(parents=True)
    subprocess.run(['git','init','--quiet',str(repo)],check=True)
    korean=chr(0xD55C)+chr(0xAE00)
    original='let original_value = "'+('old '+korean+' value with spaces ')*8+'";\n'
    (repo/'src'/'long.rs').write_text(original)
    subprocess.run(['git','-C',str(repo),'add','.'],check=True)
    subprocess.run(['git','-C',str(repo),'-c','user.name=Araseo Fixture','-c','user.email=fixture@example.invalid','commit','--quiet','-m','fixture'],check=True)
    modified='let modified_value = "'+('new '+korean+' value with spaces ')*10+'";\n'
    (repo/'src'/'long.rs').write_text(modified)
(root/'alpha'/'added.txt').write_text('new file\n')
'@
 Invoke-PrivatePython $gitFixture $workspace
}
$previousLocalAppData=$env:LOCALAPPDATA
$env:LOCALAPPDATA=Join-Path $OutputDirectory 'settings'
$process=$null
function Start-Workbench {
 $script:process=Start-Process -FilePath ([IO.Path]::GetFullPath($Executable)) -ArgumentList @('--distro',('"'+$Distro+'"'),'--root',$workspace) -RedirectStandardOutput (Join-Path $OutputDirectory 'stdout.log') -RedirectStandardError (Join-Path $OutputDirectory 'stderr.log') -PassThru
 $deadline=(Get-Date).AddSeconds(20)
 do{Start-Sleep -Milliseconds 100;$process.Refresh();if($process.HasExited){throw 'Araseo exited before showing its window'}}while($process.MainWindowHandle -eq [IntPtr]::Zero -and (Get-Date)-lt $deadline)
 if($process.MainWindowHandle -eq [IntPtr]::Zero){throw 'Araseo did not open a window'}
 $script:handle=$process.MainWindowHandle
 # GPUI obtains the taskbar icon from the executable's resource ID 1 and
 # registers it on the native window class. A missing resource returns zero.
 $iconHandle=[WorkbenchInput]::GetClassLongPtr($handle,-14)
 if($iconHandle-eq [IntPtr]::Zero){throw 'Native window class has no application/taskbar icon'}
 $iconBitmap=[Drawing.Icon]::FromHandle($iconHandle).ToBitmap()
 try{$iconBitmap.Save((Join-Path $OutputDirectory 'window-icon.png'))}finally{$iconBitmap.Dispose()}
 [WorkbenchInput]::ShowWindow($handle,9)|Out-Null
 [WorkbenchInput]::MoveWindow($handle,100,80,1180,800,$true)|Out-Null
 [WorkbenchInput]::SetForegroundWindow($handle)|Out-Null
 Start-Sleep -Milliseconds 900
}
function Assert-Foreground {if([WorkbenchInput]::GetForegroundWindow() -ne $handle){throw 'Fixture lost foreground; refusing to type into another window'}}
function Client-Point([double]$x,[double]$y){$p=New-Object WorkbenchInput+Point;$scale=[WorkbenchInput]::GetDpiForWindow($handle)/96.;$p.x=[int]($x*$scale);$p.y=[int]($y*$scale);[WorkbenchInput]::ClientToScreen($handle,[ref]$p)|Out-Null;return $p}
function Click-Client([double]$x,[double]$y){Assert-Foreground;$point=Client-Point $x $y;[WorkbenchInput]::SetCursorPos($point.x,$point.y)|Out-Null;[WorkbenchInput]::mouse_event(2,0,0,0,[UIntPtr]::Zero);[WorkbenchInput]::mouse_event(4,0,0,0,[UIntPtr]::Zero);Start-Sleep -Milliseconds 150}
function Capture([string]$name){
 Assert-Foreground;$r=New-Object WorkbenchInput+Rect;$p=New-Object WorkbenchInput+Point
 [WorkbenchInput]::GetClientRect($handle,[ref]$r)|Out-Null;[WorkbenchInput]::ClientToScreen($handle,[ref]$p)|Out-Null
 $bitmap=New-Object Drawing.Bitmap $r.r,$r.b;$graphics=[Drawing.Graphics]::FromImage($bitmap)
 try{$graphics.CopyFromScreen($p.x,$p.y,0,0,$bitmap.Size);$bitmap.Save((Join-Path $OutputDirectory $name))}finally{$graphics.Dispose();$bitmap.Dispose()}
}
function Read-Session { $file=Get-ChildItem (Join-Path $env:LOCALAPPDATA 'araseo\sessions') -Filter '*.json' -ErrorAction SilentlyContinue|Select-Object -First 1;if($file){return Get-Content -Raw $file.FullName|ConvertFrom-Json};return $null }
function Tab-Groups($node){if($null-ne $node.info.tabs){return ,$node};foreach($child in $node.children){Tab-Groups $child}}
function Invoke-Button([string]$name){$root=[Windows.Automation.AutomationElement]::FromHandle($handle);$condition=New-Object Windows.Automation.PropertyCondition ([Windows.Automation.AutomationElement]::NameProperty),$name;$button=$root.FindFirst([Windows.Automation.TreeScope]::Descendants,$condition);if(!$button){throw "Missing button $name"};$button.GetCurrentPattern([Windows.Automation.InvokePattern]::Pattern).Invoke();Start-Sleep -Milliseconds 700}
function Find-Named([string]$name){
 $root=[Windows.Automation.AutomationElement]::FromHandle($handle)
 $elements=$root.FindAll([Windows.Automation.TreeScope]::Descendants,[Windows.Automation.Condition]::TrueCondition)
 foreach($element in $elements){if($element.Current.Name.Replace('\','/') -eq $name.Replace('\','/')){return $element}}
 return $null
}
function Wait-Named([string]$name){
 $deadline=(Get-Date).AddSeconds(8)
 do{$element=Find-Named $name;if($element){return $element};Start-Sleep -Milliseconds 100}while((Get-Date)-lt $deadline)
 $root=[Windows.Automation.AutomationElement]::FromHandle($handle)
 $root.FindAll([Windows.Automation.TreeScope]::Descendants,[Windows.Automation.Condition]::TrueCondition)|ForEach-Object {Write-Output ("Accessible: "+$_.Current.Name)}|Out-Host
 throw "Missing accessible element $name"
}
function Click-Named([string]$name){
 $element=Wait-Named $name;$rect=$element.Current.BoundingRectangle
 if($rect.Width-le 0 -or $rect.Height-le 0){throw "Invisible element $name"}
 Assert-Foreground;[WorkbenchInput]::SetCursorPos([int]($rect.Left+$rect.Width/2),[int]($rect.Top+$rect.Height/2))|Out-Null
 [WorkbenchInput]::mouse_event(2,0,0,0,[UIntPtr]::Zero);[WorkbenchInput]::mouse_event(4,0,0,0,[UIntPtr]::Zero)
 Start-Sleep -Milliseconds 400
}
function Capture-AgentIcon([string]$label,[string]$name){
 Assert-Foreground
 $element=Wait-Named $label;$rect=$element.Current.BoundingRectangle
 $bitmap=New-Object Drawing.Bitmap ([int]$rect.Width),([int]$rect.Height)
 $graphics=[Drawing.Graphics]::FromImage($bitmap)
 $stream=New-Object IO.MemoryStream
 $hash=[Security.Cryptography.SHA256]::Create()
 try{
  $graphics.CopyFromScreen([int]$rect.Left,[int]$rect.Top,0,0,$bitmap.Size)
  $bitmap.Save((Join-Path $OutputDirectory $name),[Drawing.Imaging.ImageFormat]::Png)
  $bitmap.Save($stream,[Drawing.Imaging.ImageFormat]::Png)
  return [BitConverter]::ToString($hash.ComputeHash($stream.ToArray()))
 }finally{$hash.Dispose();$stream.Dispose();$graphics.Dispose();$bitmap.Dispose()}
}
function Assert-DiffColumns {
 # AccessKit exposes Label text through its value on Windows; the Kit's
 # source labels do not provide that value. Measure native Text rectangles
 # instead of relying on their empty UIA Name, without requesting a repaint.
 $deadline=(Get-Date).AddSeconds(8)
 do{
  $root=[Windows.Automation.AutomationElement]::FromHandle($handle)
  $condition=New-Object Windows.Automation.PropertyCondition ([Windows.Automation.AutomationElement]::ControlTypeProperty),([Windows.Automation.ControlType]::Text)
  $elements=$root.FindAll([Windows.Automation.TreeScope]::Descendants,$condition)
  $origin=Client-Point 0 0;$scale=[WorkbenchInput]::GetDpiForWindow($handle)/96.
  $headerTop=$origin.y+34*$scale;$headerBottom=$origin.y+64*$scale
  $headers=@($elements|ForEach-Object {$_.Current.BoundingRectangle}|Where-Object {$_.Top-ge $headerTop-2 -and $_.Bottom-le $headerBottom+2 -and $_.Width-gt 100}|Sort-Object Left)
  $body=@($elements|ForEach-Object {$_.Current.BoundingRectangle}|Where-Object {$_.Top-ge $headerBottom-2 -and $_.Width-gt 60 -and $_.Height-gt 20})
  if($headers.Count-eq 2 -and $body.Count-ge 2){break}
  Start-Sleep -Milliseconds 100
 }while((Get-Date)-lt $deadline)
 if($headers.Count-ne 2 -or $body.Count-lt 2){throw 'Diff did not paint both headers and source sides without a forced refresh'}
 $left=$headers[0];$right=$headers[1]
 if([Math]::Abs($left.Width-$right.Width)-gt 2){throw 'Diff headers do not split the panel equally'}
 $seenOld=$false;$seenNew=$false
 foreach($rect in $body){
  if($rect.Left-ge $left.Left-2 -and $rect.Right-le $left.Right+2){$seenOld=$true}
  elseif($rect.Left-ge $right.Left-2 -and $rect.Right-le $right.Right+2){$seenNew=$true}
  else{throw "Diff source overflowed its half: $rect"}
 }
 if(!$seenOld -or !$seenNew){throw 'One side of the diff is missing'}
}
function Click-Caption([int]$fromRight){
 $r=New-Object WorkbenchInput+Rect;[WorkbenchInput]::GetClientRect($handle,[ref]$r)|Out-Null
 $scale=[WorkbenchInput]::GetDpiForWindow($handle)/96.
 Click-Client ($r.r/$scale-17-34*$fromRight) 17
}
function Close-Workbench {
 if($process -and !$process.HasExited){[WorkbenchInput]::SetForegroundWindow($handle)|Out-Null;Click-Caption 0;if(!$process.WaitForExit(6000)){throw 'Araseo did not save its session and close'}}
}
try {
 Start-Workbench
 if($Mode -eq 'TerminalInput'){
  foreach($buffer in @('normal','alternate')){
   $resultPath="$workspace/input-$buffer.json"
   Click-Client 500 150
   [WorkbenchInput]::Text("HISTFILE=/dev/null; history -c; python3 $workspace/terminal-input.py $resultPath $buffer");[WorkbenchInput]::Enter()
   $deadline=(Get-Date).AddSeconds(10)
   do{Start-Sleep -Milliseconds 150;$ready=Invoke-PrivatePython 'import pathlib,sys; print(pathlib.Path(sys.argv[1]).exists())' $resultPath}while($ready-ne 'True' -and (Get-Date)-lt $deadline)
   if($ready-ne 'True'){throw "The $buffer input fixture did not start"}
   Start-Sleep -Milliseconds 300
   Assert-Foreground
   [WorkbenchInput]::Text('/per');[WorkbenchInput]::Tab($false);[WorkbenchInput]::Text('mission');[WorkbenchInput]::Tab($true)
   foreach($chord in @(@(37,1),@(39,4),@(37,2),@(37,5),@(35,7),@(13,1),@(8,4),@(46,4),@(116,1),@(82,5),@(87,4),@(83,4))){
    Assert-Foreground
    [WorkbenchInput]::Chord([uint16]$chord[0],[int]$chord[1])
   }
   Assert-Foreground
   [WorkbenchInput]::Wheel(120);Start-Sleep -Milliseconds 200;[WorkbenchInput]::Wheel(-120);Start-Sleep -Milliseconds 200
   [WorkbenchInput]::Enter()
   $deadline=(Get-Date).AddSeconds(10)
   do{Start-Sleep -Milliseconds 150;$done=Invoke-PrivatePython 'import json,pathlib,sys; print(json.loads(pathlib.Path(sys.argv[1]).read_text())["done"])' $resultPath}while($done-ne 'True' -and (Get-Date)-lt $deadline)
   if($done-ne 'True'){throw "The $buffer terminal lost keyboard focus"}
   Invoke-PrivatePython 'import json,pathlib,re,sys; d=bytes.fromhex(json.loads(pathlib.Path(sys.argv[1]).read_text())["hex"]); assert d.startswith(b"/per\tmission\x1b[Z\x1b[1;2D\x1b[1;5C\x1b[1;3D\x1b[1;6D\x1b[1;8F\x1b[13;2u\x1b[127;5u\x1b[3;5~\x1b[15;2~\x1b[114;6u\x17\x13"),d; reports=re.findall(rb"\x1b\[<(64|65);\d+;\d+M",d); assert reports and reports[0]==b"64" and reports[-1]==b"65",d; assert reports==sorted(reports),d; print("Tab, Backtab, modified keys, wheel-up and wheel-down bytes: passed")' $resultPath
   Capture "terminal-input-$buffer.png"
   Write-Output "Native terminal completion/modifier keys and wheel direction ($buffer buffer): passed"
  }
 }
 if($Mode -eq 'Agent'){
  foreach($agent in @('claude','codex')){
   $label=if($agent-eq 'claude'){'Claude'}else{'Codex'}
   Invoke-PrivatePython 'import pathlib,sys; (pathlib.Path(sys.argv[1])/"control").write_text("waiting")' $workspace
   Click-Client 500 150
   Assert-Foreground;[WorkbenchInput]::Text("HISTFILE=/dev/null; history -c; python3 $workspace/agent.py $agent");[WorkbenchInput]::Enter()
   $null=Wait-Named "$label waiting"
   $first=Capture-AgentIcon "$label waiting" "$agent-waiting-1.png"
   Start-Sleep -Milliseconds 220
   $second=Capture-AgentIcon "$label waiting" "$agent-waiting-2.png"
   if($first-ne $second){throw "$label logo moves while waiting"}
   Invoke-PrivatePython 'import pathlib,sys; (pathlib.Path(sys.argv[1])/"control").write_text("running")' $workspace
   $null=Wait-Named "$label running"
   $frames=@(for($index=0;$index-lt 6;$index++){
    Capture-AgentIcon "$label running" "$agent-running-$index.png"
    Start-Sleep -Milliseconds 90
   })
   if(@($frames|Select-Object -Unique).Count-lt 2){throw "$label logo does not rotate while running"}
   Invoke-PrivatePython 'import pathlib,sys; (pathlib.Path(sys.argv[1])/"control").write_text("waiting")' $workspace
   $null=Wait-Named "$label waiting"
   $first=Capture-AgentIcon "$label waiting" "$agent-stopped-1.png"
   Start-Sleep -Milliseconds 220
   $second=Capture-AgentIcon "$label waiting" "$agent-stopped-2.png"
   if($first-ne $second){throw "$label logo did not stop after work finished"}
   Invoke-PrivatePython 'import pathlib,sys; (pathlib.Path(sys.argv[1])/"control").write_text("exit")' $workspace
   $deadline=(Get-Date).AddSeconds(8)
   do{Start-Sleep -Milliseconds 100;$remaining=Find-Named "$label waiting"}while($remaining -and (Get-Date)-lt $deadline)
   if($remaining){throw "$label logo remains after the process exits"}
  }
  Capture 'agent-tabs.png'
  Write-Output 'Claude/Codex tab logos, pixel-changing working rotation, static waiting and process exit: passed'
 }
 if($Mode -eq 'Git'){
  Invoke-Button 'Git'
  $null=Wait-Named 'Git repository alpha';$null=Wait-Named 'Git repository beta'
  Capture 'git-repositories.png'
  Click-Named 'Git repository alpha'
  if(Find-Named "Git change $workspace/alpha/src/long.rs"){throw 'Collapsed repository still shows its changes'}
  Click-Named 'Git repository alpha'
  Click-Named "Git change $workspace/alpha/src/long.rs"
  foreach($width in @(1180,820,1180)){
   [WorkbenchInput]::MoveWindow($handle,100,80,$width,800,$true)|Out-Null;Start-Sleep -Milliseconds 500
   Assert-DiffColumns
   Capture "diff-width-$width.png"
  }
  [WorkbenchInput]::InvalidateRect($handle,[IntPtr]::Zero,$false)|Out-Null
  Start-Sleep -Milliseconds 200;Assert-DiffColumns
  Write-Output 'Repository grouping, collapse/expand, change opening and diff width after resize: passed'
 }
 if($Mode -eq 'All'){
  Invoke-Button 'New File'
  Assert-Foreground;[WorkbenchInput]::Text('header-created.txt')
  [WorkbenchInput]::Enter();Start-Sleep -Milliseconds 700
  Invoke-Button 'New Folder'
  Assert-Foreground;[WorkbenchInput]::Text('header-project')
  [WorkbenchInput]::Enter();Start-Sleep -Milliseconds 700
  Invoke-PrivatePython 'import pathlib,sys; p=pathlib.Path(sys.argv[1]); assert (p/"header-created.txt").is_file(); assert (p/"header-project").is_dir()' $workspace
  Invoke-Button 'Add Project'
  Assert-Foreground;[WorkbenchInput]::Text("$workspace/header-project")
  [WorkbenchInput]::Enter();Start-Sleep -Milliseconds 700
  $history=Get-ChildItem (Join-Path $env:LOCALAPPDATA 'araseo\workspaces') -Filter '*.conf'|Select-Object -First 1
  $encodedPath=([Text.Encoding]::UTF8.GetBytes("$workspace/header-project")|ForEach-Object {$_.ToString('x2')}) -join ''
  if(!$history -or !(Get-Content $history.FullName).Contains($encodedPath)){throw 'Add Project did not persist the selected WSL folder'}
  Capture 'compact-header.png'
  Write-Output 'Header file/folder/project icon actions: passed'
 }
 if($Mode -in @('All','Terminal')){
  Invoke-Button 'New Terminal'
  $r=New-Object WorkbenchInput+Rect;[WorkbenchInput]::GetClientRect($handle,[ref]$r)|Out-Null;$scale=[WorkbenchInput]::GetDpiForWindow($handle)/96.
  $from=Client-Point 300 17;$to=Client-Point ([Math]::Max(400,$r.r/$scale*0.65)) ($r.b/$scale-40)
  Assert-Foreground;[WorkbenchInput]::SetCursorPos($from.x,$from.y)|Out-Null
  [WorkbenchInput]::mouse_event(2,0,0,0,[UIntPtr]::Zero)
  for($step=1;$step-le 12;$step++){[WorkbenchInput]::SetCursorPos([int]($from.x+($to.x-$from.x)*$step/12),[int]($from.y+($to.y-$from.y)*$step/12))|Out-Null;Start-Sleep -Milliseconds 30}
  Start-Sleep -Milliseconds 200;[WorkbenchInput]::mouse_event(4,0,0,0,[UIntPtr]::Zero)
  Start-Sleep -Milliseconds 1600
  $session=Read-Session;$groups=@(Tab-Groups $session.gpui_layout.center)
  if($groups.Count-lt 2){throw 'Dragging the running terminal did not create a split'}
  Click-Client ([Math]::Max(400,$r.r/$scale*0.65)) ($r.b/$scale-100)
  Assert-Foreground
  [WorkbenchInput]::Text("HISTFILE=/dev/null; history -c; printf GPUI_BOTTOM_OK > $workspace/native-input.txt; printf '\nAB\355\225\234CD\n'")
  [WorkbenchInput]::Enter()
  $deadline=(Get-Date).AddSeconds(10)
  do{Start-Sleep -Milliseconds 200;$result=Invoke-PrivatePython 'import pathlib,sys; p=pathlib.Path(sys.argv[1]); print(p.read_text() if p.exists() else "")' "$workspace/native-input.txt"}while($result-ne 'GPUI_BOTTOM_OK' -and (Get-Date)-lt $deadline)
  if($result-ne 'GPUI_BOTTOM_OK'){throw 'The lower terminal did not execute continuous native input'}
  Capture 'split-terminal.png'
  Write-Output 'Native input after dragging a running terminal into the lower split: passed'
  Close-Workbench
  $before=Read-Session
  Start-Workbench
  Start-Sleep -Milliseconds 1600
  $after=Read-Session
  if(@(Tab-Groups $after.gpui_layout.center).Count -lt 2 -or $after.tabs.Count-ne $before.tabs.Count){throw 'Session layout did not restore'}
  Write-Output 'Terminal tabs and split layout restored: passed'
 }
 if($Mode -in @('All','Resize')){
  foreach($scenario in @('shrink-grow','maximize-restore','minimize-restore')){
   switch($scenario){
    'shrink-grow'{[WorkbenchInput]::MoveWindow($handle,100,80,700,500,$true)|Out-Null;Start-Sleep -Milliseconds 150;[WorkbenchInput]::MoveWindow($handle,100,80,1180,800,$true)|Out-Null}
    'maximize-restore'{Click-Caption 1;if(![WorkbenchInput]::IsZoomed($handle)){throw 'Maximize button did not maximize'};Click-Caption 1;if([WorkbenchInput]::IsZoomed($handle)){throw 'Restore button did not restore'}}
    'minimize-restore'{Click-Caption 2;if(![WorkbenchInput]::IsIconic($handle)){throw 'Minimize button did not minimize'};[WorkbenchInput]::ShowWindow($handle,9)|Out-Null}
   }
   [WorkbenchInput]::SetForegroundWindow($handle)|Out-Null
   Start-Sleep -Milliseconds 600;Capture "$scenario-before.png"
   [WorkbenchInput]::InvalidateRect($handle,[IntPtr]::Zero,$false)|Out-Null
   Start-Sleep -Milliseconds 300;Capture "$scenario-after.png"
   $before=[IO.File]::ReadAllBytes((Join-Path $OutputDirectory "$scenario-before.png"));$after=[IO.File]::ReadAllBytes((Join-Path $OutputDirectory "$scenario-after.png"))
   if([Convert]::ToBase64String($before)-ne [Convert]::ToBase64String($after)){throw "Surface changed only after repaint: $scenario"}
   Write-Output "$scenario without a deferred repaint: passed"
  }
 }
 Close-Workbench
 Write-Output 'Native GPUI workbench verification passed.'
} finally {
 if($process -and !$process.HasExited){try{Capture 'last-frame.png'}catch{}}
 if($Mode-eq 'TerminalInput' -and $process -and !$process.HasExited){try{Close-Workbench}catch{}}
 if($process){$process.Refresh();if(!$process.HasExited){Stop-Process -Id $process.Id -Force}}
 $env:LOCALAPPDATA=$previousLocalAppData
 Invoke-PrivatePython 'import shutil,sys; p=sys.argv[1]; assert p.startswith("/tmp/araseo-gpui-check-"); shutil.rmtree(p)' $workspace
}
