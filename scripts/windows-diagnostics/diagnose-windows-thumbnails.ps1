[CmdletBinding()]
param(
    [string] $DistributionDirectory = (Join-Path $PSScriptRoot '..\..\dist\release'),
    [switch] $ProbeShell,
    [ValidateSet('png', 'jpg')]
    [string] $ProbeExtension = 'png'
)

$ErrorActionPreference = 'Stop'
if ([IntPtr]::Size -ne 8) { throw 'Please use 64-bit Windows PowerShell.' }

$typeInitializationError = $null
try {
    Add-Type -ReferencedAssemblies System.Drawing.dll -TypeDefinition @'
using System;
using System.Drawing;
using System.Drawing.Imaging;
using System.Runtime.InteropServices;
using System.Text;

public static class ThumbnailDiagnostic
{
    // Vtable order and flags: Windows SDK thumbcache.h / IThumbnailCache.GetThumbnail.
    // https://learn.microsoft.com/windows/win32/api/thumbcache/nf-thumbcache-ithumbnailcache-getthumbnail
    [StructLayout(LayoutKind.Sequential)]
    private struct Size { public int Width, Height; }
    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    private delegate int GetThumbnail(IntPtr self, IntPtr item, uint size, uint flags,
        out IntPtr bitmap, out uint cacheFlags, IntPtr thumbnailId);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    private delegate int GetSize(IntPtr self, out Size size);
    [DllImport("ole32.dll")]
    private static extern int CoInitializeEx(IntPtr reserved, uint flags);
    [DllImport("ole32.dll")]
    private static extern void CoUninitialize();
    [DllImport("ole32.dll")]
    private static extern int CoCreateInstance(ref Guid clsid, IntPtr outer, uint context,
        ref Guid iid, out IntPtr instance);
    [DllImport("shell32.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
    private static extern int SHCreateItemFromParsingName(string path, IntPtr context,
        ref Guid iid, out IntPtr item);
    [DllImport("shlwapi.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
    private static extern int AssocQueryStringW(uint flags, uint kind, string extension,
        string extra, StringBuilder output, ref uint length);

    private static bool Report(int hr, string stage)
    {
        Console.WriteLine(stage + ": HRESULT 0x" + hr.ToString("X8"));
        return hr >= 0;
    }
    private static Delegate Method(IntPtr instance, int slot, Type type)
    {
        return Marshal.GetDelegateForFunctionPointer(
            Marshal.ReadIntPtr(Marshal.ReadIntPtr(instance), slot * IntPtr.Size), type);
    }
    public static string Handler(string extension)
    {
        uint length = 512;
        StringBuilder output = new StringBuilder((int)length);
        int hr = AssocQueryStringW(0, 16, extension,
            "{E357FCCD-A995-4576-B01F-234630154E96}", output, ref length);
        return "HRESULT 0x" + hr.ToString("X8") + (hr >= 0 ? "; " + output : "");
    }
    public static void CreateImage(string path, bool jpeg)
    {
        using (Bitmap image = new Bitmap(320, 160, PixelFormat.Format24bppRgb))
        {
            using (Graphics graphics = Graphics.FromImage(image))
                graphics.Clear(Color.FromArgb(30, 180, 100));
            image.Save(path, jpeg ? ImageFormat.Jpeg : ImageFormat.Png);
        }
    }
    public static void Probe(string path, string label)
    {
        uint model = System.Threading.Thread.CurrentThread.GetApartmentState()
            == System.Threading.ApartmentState.MTA ? 0u : 2u;
        int apartment = CoInitializeEx(IntPtr.Zero, model);
        IntPtr cache = IntPtr.Zero, item = IntPtr.Zero, bitmap = IntPtr.Zero;
        try
        {
            if (!Report(apartment, label + " CoInitializeEx")
                && apartment != unchecked((int)0x80010106)) return;
            Guid clsid = new Guid("50EF4544-AC9F-4A8E-B21B-8A26180DB13F");
            Guid cacheId = new Guid("F676C15D-596A-4CE2-8234-33996F445DB1");
            if (!Report(CoCreateInstance(ref clsid, IntPtr.Zero, 1, ref cacheId, out cache),
                label + " CoCreateInstance(LocalThumbnailCache)")) return;
            Guid itemId = new Guid("43826D1E-E718-42EE-BC55-A1E261C37BFE");
            if (!Report(SHCreateItemFromParsingName(path, IntPtr.Zero, ref itemId, out item),
                label + " SHCreateItemFromParsingName")) return;
            GetThumbnail extract = (GetThumbnail)Method(cache, 3, typeof(GetThumbnail));
            uint flags;
            Console.WriteLine(label + " starting isolated Shell extraction (WTS_FORCEEXTRACTION)");
            if (!Report(extract(cache, item, 96, 4, out bitmap, out flags, IntPtr.Zero),
                label + " IThumbnailCache.GetThumbnail")) return;
            if (bitmap == IntPtr.Zero) { Console.WriteLine(label + " ERROR: null bitmap"); return; }
            Size dimensions;
            GetSize getSize = (GetSize)Method(bitmap, 4, typeof(GetSize));
            if (Report(getSize(bitmap, out dimensions), label + " ISharedBitmap.GetSize"))
                Console.WriteLine(label + " bitmap " + dimensions.Width + "x" + dimensions.Height
                    + "; cache flags 0x" + flags.ToString("X8"));
        }
        finally
        {
            if (bitmap != IntPtr.Zero) Marshal.Release(bitmap);
            if (item != IntPtr.Zero) Marshal.Release(item);
            if (cache != IntPtr.Zero) Marshal.Release(cache);
            if (apartment >= 0) CoUninitialize();
        }
    }
}
'@
} catch {
    $typeInitializationError = $_.Exception
}

if ($ProbeShell) {
    if ($typeInitializationError) { throw $typeInitializationError }
    $temporary = Join-Path ([IO.Path]::GetTempPath()) ('vmv-thumbnail-' + [Guid]::NewGuid().ToString('N'))
    [void] (New-Item -ItemType Directory -Path $temporary)
    try {
        $source = Join-Path $temporary ('test.' + $ProbeExtension)
        [ThumbnailDiagnostic]::CreateImage($source, ($ProbeExtension -eq 'jpg'))
        [ThumbnailDiagnostic]::Probe($source, $ProbeExtension)
    } finally {
        Get-ChildItem -LiteralPath $temporary -File | Remove-Item -Force
        Remove-Item -LiteralPath $temporary -Force
    }
    exit
}

$log = New-Object 'System.Collections.Generic.List[string]'
function Write-Report([string] $Message) {
    if ($env:USERPROFILE) { $Message = $Message.Replace($env:USERPROFILE, '%USERPROFILE%') }
    $log.Add($Message)
    Write-Host $Message
}
function Read-RegistryValue([string] $Path, [string] $Name = '') {
    $key = Get-Item -LiteralPath $Path -ErrorAction SilentlyContinue
    if ($null -eq $key) { return '<not set>' }
    $value = $key.GetValue($Name, $null)
    if ($null -eq $value) { return '<not set>' }
    return [string] $value
}
function Quote-Argument([string] $Value) { return "'" + $Value.Replace("'", "''") + "'" }
function Invoke-Probe([string] $Label, [string] $Command) {
    Write-Report ("`r`n=== " + $Label + ' ===')
    $start = New-Object System.Diagnostics.ProcessStartInfo
    $start.FileName = Join-Path $PSHOME 'powershell.exe'
    $wrapped = "`$ErrorActionPreference = 'Stop'; `$ProgressPreference = 'SilentlyContinue'; try { " + $Command + " } catch { Write-Output (`$_.Exception.ToString()); exit 1 }"
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($wrapped))
    $start.Arguments = '-MTA -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand ' + $encoded
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $start
    try {
        [void] $process.Start()
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(30000)) {
            # This is only the diagnostic's own child, never Explorer or its surrogate.
            $process.Kill()
            $process.WaitForExit()
            Write-Report 'TIMEOUT: diagnostic child stopped after 30 seconds.'
        }
        Write-Report ('Child exit code: ' + $process.ExitCode)
        Write-Report ($stdout.GetAwaiter().GetResult())
        $errorOutput = $stderr.GetAwaiter().GetResult()
        if ($errorOutput) { Write-Report ('stderr: ' + $errorOutput) }
    } catch {
        Write-Report ('Probe launch failed: ' + $_.Exception.Message)
    } finally { $process.Dispose() }
}

try {
    Write-Report 'VisualMediaViewer thumbnail diagnostic (read-only settings; generated test files only)'
    Write-Report ('Date: ' + [DateTime]::Now.ToString('s'))
    Write-Report ('OS: ' + [Environment]::OSVersion.VersionString)
    Write-Report ('64-bit OS: ' + [Environment]::Is64BitOperatingSystem + '; process: ' + ([IntPtr]::Size * 8) + '-bit')
    Write-Report ('PowerShell: ' + $PSVersionTable.PSVersion)
    if ($typeInitializationError) { throw $typeInitializationError }
    $class = 'Registry::HKEY_CURRENT_USER\Software\Classes\CLSID\{D4F66B4E-5762-4BA9-9633-138915FD01A7}'
    $provider = Read-RegistryValue ($class + '\InprocServer32')
    $providerExists = ($provider -ne '<not set>') -and (Test-Path -LiteralPath $provider -PathType Leaf)
    Write-Report ('Registered provider: ' + $provider)
    Write-Report ('Provider exists: ' + $providerExists)
    Write-Report ('ThreadingModel: ' + (Read-RegistryValue ($class + '\InprocServer32') 'ThreadingModel'))
    Write-Report ('AppID: ' + (Read-RegistryValue $class 'AppID'))
    foreach ($extension in @('.png', '.jpg', '.jpeg', '.mp4', '.webm')) {
        Write-Report ($extension + ' effective thumbnail handler: ' + [ThumbnailDiagnostic]::Handler($extension))
    }
    foreach ($scope in @('HKEY_CURRENT_USER', 'HKEY_LOCAL_MACHINE')) {
        Write-Report ($scope + ' Shell Extensions Blocked: ' + (Read-RegistryValue (
            'Registry::' + $scope + '\Software\Microsoft\Windows\CurrentVersion\Shell Extensions\Blocked'
        ) '{D4F66B4E-5762-4BA9-9633-138915FD01A7}'))
        foreach ($setting in @(
            @('Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced', 'IconsOnly'),
            @('Software\Microsoft\Windows\CurrentVersion\Policies\Explorer', 'DisableThumbnails'),
            @('Software\Microsoft\Windows\CurrentVersion\Policies\Explorer', 'DisableThumbnailsOnNetworkFolders'),
            @('Software\Policies\Microsoft\Windows\Explorer', 'DisableThumbnails'),
            @('Software\Policies\Microsoft\Windows\Explorer', 'DisableThumbnailsOnNetworkFolders')
        )) {
            Write-Report ($scope + '\' + $setting[0] + ' [' + $setting[1] + ']: ' +
                (Read-RegistryValue ('Registry::' + $scope + '\' + $setting[0]) $setting[1]))
        }
    }
    $directories = @([PSCustomObject] @{ Label = 'Portable'; Path = $DistributionDirectory })
    if ($providerExists) {
        $directories += [PSCustomObject] @{ Label = 'Registered'; Path = Split-Path -Parent $provider }
    }
    $helper = Join-Path $PSScriptRoot 'test-windows-shell.ps1'
    foreach ($entry in $directories) {
        Write-Report ("`r`n" + $entry.Label + ' components:')
        foreach ($name in @('VisualMediaViewer.exe', 'VisualMediaViewerThumbnail.dll', 'avcodec-61.dll',
            'avdevice-61.dll', 'avfilter-10.dll', 'avformat-61.dll', 'avutil-59.dll', 'swresample-5.dll', 'swscale-8.dll')) {
            $file = Join-Path $entry.Path $name
            if (Test-Path -LiteralPath $file -PathType Leaf) {
                Write-Report ($name + ': present; SHA256 ' + (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash)
            } else { Write-Report ($name + ': MISSING') }
        }
        if (Test-Path -LiteralPath $helper -PathType Leaf) {
            $command = '& ' + (Quote-Argument $helper) + ' -DistributionDirectory ' + (Quote-Argument $entry.Path)
            Invoke-Probe ($entry.Label + ' direct DLL PNG / malformed input') $command
            Invoke-Probe ($entry.Label + ' native-only DLL PNG + JPEG') ($command + ' -NativeImagesOnly')
        } else { Write-Report 'MISSING test-windows-shell.ps1: direct DLL probes skipped.' }
    }
    foreach ($extension in @('png', 'jpg')) {
        Invoke-Probe ('Real Windows Shell ' + $extension + ' (MTA; isolated extraction, not icons)') (
            '& ' + (Quote-Argument $PSCommandPath) + ' -ProbeShell -ProbeExtension ' + $extension)
    }
    Write-Report "`r`nNo settings were changed, no global cache was deleted, and Explorer was not terminated."
} catch {
    Write-Report ('Diagnostic error: ' + $_.Exception.ToString())
} finally {
    $output = Join-Path $PSScriptRoot 'thumbnail-diagnostic.txt'
    [IO.File]::WriteAllLines($output, $log.ToArray(), (New-Object Text.UTF8Encoding($true)))
    Write-Host 'Report written: thumbnail-diagnostic.txt'
}
