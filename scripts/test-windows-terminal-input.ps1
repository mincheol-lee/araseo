param([Parameter(Mandatory=$true)][string]$Executable,[Parameter(Mandatory=$true)][string]$OutputDirectory)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class FixtureInput {
 [StructLayout(LayoutKind.Sequential)] public struct Point { public int x,y; }
 [StructLayout(LayoutKind.Sequential)] public struct Rect { public int l,t,r,b; }
 [StructLayout(LayoutKind.Sequential)] public struct Keyboard { public ushort vk,scan; public uint flags,time; public UIntPtr extra; }
 [StructLayout(LayoutKind.Explicit,Size=40)] public struct Input { [FieldOffset(0)] public uint type; [FieldOffset(8)] public Keyboard key; }
 [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
 [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
 [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h,out Rect r);
 [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h,ref Point p);
 [DllImport("user32.dll")] static extern uint SendInput(uint count,Input[] inputs,int size);
 public static void Type(char c) {
  var a=new Input[2];a[0].type=1;a[0].key.vk=(ushort)char.ToUpperInvariant(c);a[0].key.flags=0;
  a[1]=a[0];a[1].key.flags=2;
  if(SendInput(2,a,40)!=2)throw new Exception("SendInput failed");
 }
}
'@
New-Item -ItemType Directory -Force $OutputDirectory | Out-Null
foreach($mode in @('bottom','extra')) {
 $log=Join-Path $OutputDirectory "$mode.log"
 Remove-Item -ErrorAction SilentlyContinue $log
 $p=Start-Process -FilePath $Executable -ArgumentList @($mode,('"'+$log+'"')) -PassThru
 try {
  $deadline=(Get-Date).AddSeconds(15)
  do {
   Start-Sleep -Milliseconds 100
   $p.Refresh()
   if($p.HasExited){throw 'Fixture exited'}
   $ready=(Test-Path $log) -and (Get-Content -Raw $log).Contains('READY')
  } while((!$ready -or $p.MainWindowTitle -ne 'Araseo') -and (Get-Date) -lt $deadline)
  if(!$ready -or $p.MainWindowTitle -ne 'Araseo'){throw 'Fixture not ready'}
  $hwnd=$p.MainWindowHandle
  [FixtureInput]::SetForegroundWindow($hwnd)|Out-Null
  Start-Sleep -Milliseconds 600
  foreach($c in 'abcdef'.ToCharArray()) {
   if([FixtureInput]::GetForegroundWindow() -ne $hwnd){throw 'Fixture lost native focus; refusing to type into another window'}
   [FixtureInput]::Type($c)
   Start-Sleep -Milliseconds 300
  }
  $deadline=(Get-Date).AddSeconds(8)
  do {
   Start-Sleep -Milliseconds 100
   $text=Get-Content -Raw $log
   $complete=$text.Contains('TEXT 6 2') -and (!$text.Contains('REAL_CODEX') -or $text.Contains('COMPOSER abcdef'))
  } while(!$complete -and (Get-Date) -lt $deadline)
  if(!$complete){throw "Continuous input did not reach the terminal application: $text"}
  if([FixtureInput]::GetForegroundWindow() -ne $hwnd){throw 'Fixture lost foreground before capture'}
  $rect=New-Object FixtureInput+Rect
  $origin=New-Object FixtureInput+Point
  if(![FixtureInput]::GetClientRect($hwnd,[ref]$rect) -or ![FixtureInput]::ClientToScreen($hwnd,[ref]$origin)){throw 'Could not locate fixture surface'}
  $image=New-Object System.Drawing.Bitmap(($rect.r-$rect.l),($rect.b-$rect.t))
  $graphics=[System.Drawing.Graphics]::FromImage($image)
  try {
   $graphics.CopyFromScreen($origin.x,$origin.y,0,0,$image.Size)
   $image.Save((Join-Path $OutputDirectory "$mode.png"),[System.Drawing.Imaging.ImageFormat]::Png)
  } finally {$graphics.Dispose();$image.Dispose()}
  Write-Output "$mode`n$text"
 } finally {
  if(!$p.HasExited){$p.CloseMainWindow()|Out-Null;if(!$p.WaitForExit(4000)){Stop-Process -Id $p.Id -Force}}
 }
}
