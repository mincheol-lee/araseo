param(
    [Parameter(Mandatory=$true)][string]$Executable,
    [Parameter(Mandatory=$true)][string]$OutputDirectory
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
Add-Type -ReferencedAssemblies System.Drawing -TypeDefinition @'
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
public static class ResizeCapture {
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool MoveWindow(IntPtr hwnd, int x, int y, int w, int h, bool repaint);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hwnd, int command);
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hwnd, out Rect rect);
    [DllImport("user32.dll")] public static extern IntPtr GetDC(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern int ReleaseDC(IntPtr hwnd, IntPtr dc);
    [DllImport("gdi32.dll")] public static extern bool BitBlt(IntPtr dst, int x, int y, int w, int h, IntPtr src, int sx, int sy, uint op);
    [DllImport("dwmapi.dll")] public static extern int DwmFlush();
    [DllImport("user32.dll")] public static extern bool InvalidateRect(IntPtr hwnd, IntPtr rect, bool erase);
    public static void DamageClientAndExpose(IntPtr hwnd) {
        // Simulate native surface loss while the renderer's cached pixels stay valid.
        var dc = GetDC(hwnd);
        try {
            using (var graphics = Graphics.FromHdc(dc)) {
                graphics.FillRectangle(Brushes.Magenta, 20, 20, 120, 80);
            }
        } finally { ReleaseDC(hwnd, dc); }
        InvalidateRect(hwnd, IntPtr.Zero, false);
    }
    public static Bitmap Capture(IntPtr hwnd) {
        DwmFlush();
        Rect r;
        if (!GetClientRect(hwnd, out r)) throw new Exception("Cannot read client size");
        var bmp = new Bitmap(r.Right, r.Bottom, PixelFormat.Format32bppRgb);
        var src = GetDC(hwnd);
        try {
            using (var graphics = Graphics.FromImage(bmp)) {
                var dst = graphics.GetHdc();
                try {
                    if (!BitBlt(dst, 0, 0, bmp.Width, bmp.Height, src, 0, 0, 0x00CC0020)) throw new Exception("Client capture failed");
                } finally { graphics.ReleaseHdc(dst); }
            }
        } finally { ReleaseDC(hwnd, src); }
        return bmp;
    }
    public static void ValidateCapture(Bitmap a) {
        int visible = 0;
        for (int y=0; y<a.Height; y+=8) for (int x=0; x<a.Width; x+=8) {
            var c = a.GetPixel(x,y); if (c.R!=0 || c.G!=0 || c.B!=0) visible++;
        }
        if (visible < (a.Width/8)*(a.Height/8)/2) throw new Exception("Capture is blank or obscured; this is not a valid UI verification");
    }
    public static int Differences(Bitmap a, Bitmap b) {
        if (a.Size != b.Size) throw new Exception("Client size did not return to its original dimensions");
        var rect = new Rectangle(0, 0, a.Width, a.Height);
        var da = a.LockBits(rect, ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
        var db = b.LockBits(rect, ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
        try {
            var pa = new byte[da.Stride * a.Height]; var pb = new byte[db.Stride * b.Height];
            Marshal.Copy(da.Scan0, pa, 0, pa.Length); Marshal.Copy(db.Scan0, pb, 0, pb.Length);
            int count = 0;
            for (int i=0; i<pa.Length; i+=4) if (pa[i]!=pb[i] || pa[i+1]!=pb[i+1] || pa[i+2]!=pb[i+2]) count++;
            return count;
        } finally { a.UnlockBits(da); b.UnlockBits(db); }
    }
}
'@
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$env:SLINT_BACKEND = 'winit-software'
$process = Start-Process -FilePath $Executable -PassThru
$baseline = $null
try {
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    do {
        Start-Sleep -Milliseconds 100
        $process.Refresh()
        if ($process.HasExited) { throw 'Native fixture exited before opening its window' }
        $handle = $process.MainWindowHandle
    } while (($handle -eq [IntPtr]::Zero -or $process.MainWindowTitle -ne "Araseo") -and [DateTime]::UtcNow -lt $deadline)
    if ($handle -eq [IntPtr]::Zero -or $process.MainWindowTitle -ne "Araseo") { throw 'Native Araseo window did not open' }
    [void][ResizeCapture]::SetForegroundWindow($handle)
    if (![ResizeCapture]::MoveWindow($handle, 120, 100, 1000, 700, $true)) { throw 'Initial resize failed' }
    Write-Output ("Fixture PID {0}, HWND {1}, title '{2}'" -f $process.Id, $handle, $process.MainWindowTitle)
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    do {
        Start-Sleep -Milliseconds 250
        $baseline = [ResizeCapture]::Capture($handle)
        try { [ResizeCapture]::ValidateCapture($baseline); break } catch {
            $baseline.Dispose(); $baseline = $null
            if ([DateTime]::UtcNow -ge $deadline) { throw }
        }
    } while ($true)
    $baseline.Save((Join-Path $OutputDirectory 'baseline.png'))
    foreach ($scenario in @('shrink-grow', 'rapid-shrink-grow', 'maximize-restore', 'minimize-restore', 'native-surface-loss')) {
        switch ($scenario) {
            'native-surface-loss' {
                for ($expose = 0; $expose -lt 3; $expose++) {
                    [ResizeCapture]::DamageClientAndExpose($handle)
                    Start-Sleep -Milliseconds 200
                }
            }
            'shrink-grow' {
                [void][ResizeCapture]::MoveWindow($handle, 120, 100, 700, 500, $true)
                Start-Sleep -Milliseconds 400
                [void][ResizeCapture]::MoveWindow($handle, 120, 100, 1000, 700, $true)
            }
            'rapid-shrink-grow' {
                for ($cycle = 0; $cycle -lt 8; $cycle++) {
                    [void][ResizeCapture]::MoveWindow($handle, 120, 100, 700, 500, $true)
                    [void][ResizeCapture]::MoveWindow($handle, 120, 100, 1000, 700, $true)
                }
            }
            'maximize-restore' {
                [void][ResizeCapture]::ShowWindow($handle, 3)
                Start-Sleep -Milliseconds 400
                [void][ResizeCapture]::ShowWindow($handle, 9)
            }
            'minimize-restore' {
                [void][ResizeCapture]::ShowWindow($handle, 6)
                Start-Sleep -Milliseconds 400
                [void][ResizeCapture]::ShowWindow($handle, 9)
            }
        }
        Start-Sleep -Milliseconds 900
        $actual = [ResizeCapture]::Capture($handle)
        try {
            $actual.Save((Join-Path $OutputDirectory "$scenario.png"))
            $different = [ResizeCapture]::Differences($baseline, $actual)
            Write-Output "$scenario`: $different changed pixels"
            if ($different -ne 0) { throw "Native client pixels remain stale after $scenario" }
        } finally { $actual.Dispose() }
    }
    Write-Output 'Native Windows resize verification passed.'
} finally {
    if ($null -ne $baseline) { $baseline.Dispose() }
    # This test only terminates its own disposable fixture, never the user's Araseo.
    $process.Refresh()
    if (!$process.HasExited) { Stop-Process -Id $process.Id -Force }
}
