[CmdletBinding()]
param(
    [string] $DistributionDirectory = (Join-Path $PSScriptRoot '..\..\dist\release'),
    [string] $MediaPath,
    [switch] $NativeImagesOnly
)

$ErrorActionPreference = 'Stop'
if ([IntPtr]::Size -ne 8) {
    throw 'Run this test in 64-bit Windows PowerShell. The thumbnail DLL is x64.'
}

$distribution = (Resolve-Path -LiteralPath $DistributionDirectory).ProviderPath
$provider = Join-Path $distribution 'VisualMediaViewerThumbnail.dll'
$requiredFiles = @($provider)
if (-not $NativeImagesOnly) {
    $requiredFiles += Join-Path $distribution 'VisualMediaViewer.exe'
}
foreach ($required in $requiredFiles) {
    if (-not (Test-Path -LiteralPath $required -PathType Leaf)) {
        throw "Required distribution file is missing: $required"
    }
}

Add-Type -AssemblyName System.Drawing
if (-not ('VisualMediaViewerShellSmoke' -as [type])) {
    Add-Type -ReferencedAssemblies System.Drawing.dll -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Diagnostics;
using System.Drawing;
using System.Drawing.Imaging;
using System.IO;
using System.Runtime.InteropServices;

public static class VisualMediaViewerShellSmoke
{
    private static readonly Guid ClassId = new Guid("D4F66B4E-5762-4BA9-9633-138915FD01A7");
    private static readonly Guid ClassFactoryId = new Guid("00000001-0000-0000-C000-000000000046");
    private static readonly Guid ThumbnailId = new Guid("E357FCCD-A995-4576-B01F-234630154E96");
    private static readonly Guid StreamInitializerId = new Guid("B824B49D-22AC-4161-AC8A-9916E8FA3F7F");

    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    private delegate int GetClassObject(ref Guid classId, ref Guid interfaceId, out IntPtr result);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    private delegate int CanUnloadNow();
    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    private delegate int CreateInstance(IntPtr self, IntPtr outer, ref Guid interfaceId, out IntPtr result);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    private delegate int InitializeStream(IntPtr self, IntPtr stream, uint mode);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    private delegate int GetThumbnail(IntPtr self, uint size, out IntPtr bitmap, out uint alpha);
    [UnmanagedFunctionPointer(CallingConvention.StdCall)]
    private delegate int SeekStream(IntPtr self, long move, uint origin, out ulong position);

    [StructLayout(LayoutKind.Sequential)]
    private struct NativeBitmap
    {
        public int Type, Width, Height, WidthBytes;
        public ushort Planes, BitsPixel;
        public IntPtr Bits;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, ExactSpelling = true, SetLastError = true)]
    private static extern IntPtr LoadLibraryW(string path);
    [DllImport("kernel32.dll", CharSet = CharSet.Ansi, ExactSpelling = true, SetLastError = true)]
    private static extern IntPtr GetProcAddress(IntPtr module, string name);
    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool FreeLibrary(IntPtr module);
    [DllImport("ole32.dll")]
    private static extern int CoInitializeEx(IntPtr reserved, uint mode);
    [DllImport("ole32.dll")]
    private static extern void CoUninitialize();
    [DllImport("shlwapi.dll", CharSet = CharSet.Unicode, ExactSpelling = true)]
    private static extern int SHCreateStreamOnFileEx(string file, uint mode, uint attributes,
        [MarshalAs(UnmanagedType.Bool)] bool create, IntPtr template, out IntPtr stream);
    [DllImport("gdi32.dll", EntryPoint = "GetObjectW", ExactSpelling = true, SetLastError = true)]
    private static extern int GetBitmapObject(IntPtr bitmap, int size, out NativeBitmap result);
    [DllImport("gdi32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool DeleteObject(IntPtr bitmap);

    private static void Check(int hr, string operation)
    {
        if (hr < 0)
            throw new InvalidOperationException(operation + " failed: HRESULT 0x" + hr.ToString("X8")
                + ". Verify the complete EXE/DLL distribution is present and the source format is supported.");
    }

    private static Delegate Export(IntPtr module, string name, Type type)
    {
        IntPtr address = GetProcAddress(module, name);
        if (address == IntPtr.Zero)
            throw new InvalidOperationException("Missing DLL export " + name + ": "
                + new Win32Exception(Marshal.GetLastWin32Error()).Message);
        return Marshal.GetDelegateForFunctionPointer(address, type);
    }

    private static Delegate Method(IntPtr instance, int slot, Type type)
    {
        IntPtr table = Marshal.ReadIntPtr(instance);
        return Marshal.GetDelegateForFunctionPointer(Marshal.ReadIntPtr(table, slot * IntPtr.Size), type);
    }

    private static void Release(ref IntPtr instance)
    {
        if (instance != IntPtr.Zero)
        {
            Marshal.Release(instance);
            instance = IntPtr.Zero;
        }
    }

    private static void CheckApartmentBound(IntPtr instance, string name)
    {
        Guid[] forbidden = {
            new Guid("94EA2B94-E9CC-49E0-C0FF-EE64CA8F5B90"), // IAgileObject
            new Guid("00000003-0000-0000-C000-000000000046")  // IMarshal
        };
        foreach (Guid interfaceId in forbidden)
        {
            Guid iid = interfaceId;
            IntPtr result = IntPtr.Zero;
            try
            {
                int hr = Marshal.QueryInterface(instance, ref iid, out result);
                if (hr != unchecked((int)0x80004002) || result != IntPtr.Zero)
                    throw new InvalidOperationException(name + " must use standard apartment marshaling; QI "
                        + iid + " returned HRESULT 0x" + hr.ToString("X8") + ".");
            }
            finally { Release(ref result); }
        }
    }

    public static void CreateAlphaPng(string path)
    {
        using (Bitmap image = new Bitmap(320, 160, PixelFormat.Format32bppArgb))
        {
            using (Graphics graphics = Graphics.FromImage(image))
                graphics.Clear(Color.FromArgb(128, 200, 100, 50));
            image.Save(path, ImageFormat.Png);
        }
    }

    public static void CreateJpeg(string path)
    {
        using (Bitmap image = new Bitmap(320, 160, PixelFormat.Format24bppRgb))
        {
            using (Graphics graphics = Graphics.FromImage(image))
                graphics.Clear(Color.FromArgb(200, 100, 50));
            image.Save(path, ImageFormat.Jpeg);
        }
    }

    private static string RunCase(GetClassObject getClass, CanUnloadNow canUnload,
        string path, bool checkAlpha, bool expectFailure)
    {
        IntPtr factory = IntPtr.Zero, thumbnail = IntPtr.Zero, initializer = IntPtr.Zero;
        IntPtr stream = IntPtr.Zero, bitmap = IntPtr.Zero;
        string description;
        try
        {
            Guid classId = ClassId, factoryId = ClassFactoryId, thumbnailId = ThumbnailId;
            Check(getClass(ref classId, ref factoryId, out factory), "DllGetClassObject");
            if (factory == IntPtr.Zero) throw new InvalidOperationException("Factory pointer is null.");
            CheckApartmentBound(factory, "Class factory");
            CreateInstance create = (CreateInstance)Method(factory, 3, typeof(CreateInstance));
            Check(create(factory, IntPtr.Zero, ref thumbnailId, out thumbnail), "IClassFactory.CreateInstance");
            if (thumbnail == IntPtr.Zero) throw new InvalidOperationException("Thumbnail provider pointer is null.");
            CheckApartmentBound(thumbnail, "Thumbnail provider");
            Guid initializerId = StreamInitializerId;
            Check(Marshal.QueryInterface(thumbnail, ref initializerId, out initializer), "QueryInterface(IInitializeWithStream)");
            Check(SHCreateStreamOnFileEx(path, 0x40, 0, false, IntPtr.Zero, out stream), "Open test source stream");
            InitializeStream initialize = (InitializeStream)Method(initializer, 3, typeof(InitializeStream));
            Check(initialize(initializer, stream, 0), "IInitializeWithStream.Initialize");
            if (canUnload() != 1)
                throw new InvalidOperationException("DllCanUnloadNow must return S_FALSE while COM objects are alive.");

            GetThumbnail getThumbnail = (GetThumbnail)Method(thumbnail, 3, typeof(GetThumbnail));
            SeekStream seek = (SeekStream)Method(stream, 5, typeof(SeekStream));
            ulong position;
            Check(seek(stream, 3, 0, out position), "Position source stream");
            uint alpha;
            Stopwatch timer = Stopwatch.StartNew();
            int hr = getThumbnail(thumbnail, 64, out bitmap, out alpha);
            timer.Stop();
            Check(seek(stream, 0, 1, out position), "Inspect source stream position");
            if (position != 3)
                throw new InvalidOperationException("Source stream position was not restored.");
            if (expectFailure)
            {
                if (hr >= 0 || bitmap != IntPtr.Zero)
                    throw new InvalidOperationException("Malformed source must fail and return no HBITMAP.");
                description = "Malformed source: rejected, no bitmap (0x" + hr.ToString("X8") + ").";
            }
            else
            {
                Check(hr, "IThumbnailProvider.GetThumbnail");
                if (bitmap == IntPtr.Zero) throw new InvalidOperationException("Successful response has no HBITMAP.");
                NativeBitmap info;
                if (GetBitmapObject(bitmap, Marshal.SizeOf(typeof(NativeBitmap)), out info) == 0)
                    throw new Win32Exception(Marshal.GetLastWin32Error(), "GetObject(HBITMAP) failed.");
                if (info.Width < 1 || info.Width > 64 || info.Height < 1 || info.Height > 64
                    || info.BitsPixel != 32 || info.Planes != 1 || info.Bits == IntPtr.Zero
                    || info.WidthBytes < info.Width * 4 || alpha != 2)
                    throw new InvalidOperationException("Invalid bitmap dimensions, pixel format, storage, or alpha type.");
                if (checkAlpha)
                {
                    if (info.Width != 2 * info.Height)
                        throw new InvalidOperationException("The 2:1 source aspect ratio was not preserved.");
                    int offset = (info.Height / 2) * info.WidthBytes + (info.Width / 2) * 4;
                    byte[] pixel = new byte[4];
                    Marshal.Copy(IntPtr.Add(info.Bits, offset), pixel, 0, 4);
                    int[] expected = { 25, 50, 100, 128 };
                    for (int channel = 0; channel < 4; channel++)
                        if (Math.Abs((int)pixel[channel] - expected[channel]) > 1)
                            throw new InvalidOperationException("Expected premultiplied BGRA [25,50,100,128], received ["
                                + String.Join(",", Array.ConvertAll(pixel, x => x.ToString())) + "].");
                }
                description = "Thumbnail: " + info.Width + "x" + info.Height + ", 32-bit ARGB"
                    + (checkAlpha ? "; aspect ratio and premultiplied alpha verified." : ".")
                    + " Generation: " + timer.Elapsed.TotalMilliseconds.ToString("F1") + " ms.";
            }
        }
        finally
        {
            if (bitmap != IntPtr.Zero) DeleteObject(bitmap);
            Release(ref initializer);
            Release(ref thumbnail);
            Release(ref stream);
            Release(ref factory);
        }
        if (canUnload() != 0)
            throw new InvalidOperationException("DllCanUnloadNow did not return S_OK after releasing every COM reference.");
        return description;
    }

    public static string Run(string provider, string media, string malformed, bool checkAlpha)
    {
        int apartment = CoInitializeEx(IntPtr.Zero, 2);
        // Existing PowerShell COM apartments may already use another model.
        if (apartment < 0 && apartment != unchecked((int)0x80010106)) Check(apartment, "CoInitializeEx");
        IntPtr module = IntPtr.Zero;
        CanUnloadNow canUnload = null;
        try
        {
            module = LoadLibraryW(provider);
            if (module == IntPtr.Zero)
                throw new Win32Exception(Marshal.GetLastWin32Error(), "Cannot load " + provider);
            GetClassObject getClass = (GetClassObject)Export(module, "DllGetClassObject", typeof(GetClassObject));
            canUnload = (CanUnloadNow)Export(module, "DllCanUnloadNow", typeof(CanUnloadNow));
            if (canUnload() != 0) throw new InvalidOperationException("Fresh DLL unexpectedly has live COM objects.");
            string valid = RunCase(getClass, canUnload, media, checkAlpha, false);
            string invalid = RunCase(getClass, canUnload, malformed, false, true);
            return valid + Environment.NewLine + invalid + Environment.NewLine
                + "Apartment-bound COM interfaces verified; objects released; DLL unloadable. No registry registration was performed.";
        }
        finally
        {
            // Never unload executable code while a COM object may still reference it.
            if (module != IntPtr.Zero && (canUnload == null || canUnload() == 0)) FreeLibrary(module);
            if (apartment >= 0) CoUninitialize();
        }
    }
}
'@
}

$temporaryDirectory = Join-Path $distribution ('.shell-smoke-' + [Guid]::NewGuid().ToString('N'))
[void] (New-Item -ItemType Directory -Path $temporaryDirectory)
try {
    if ($NativeImagesOnly) {
        $nativeProvider = Join-Path $temporaryDirectory 'VisualMediaViewerThumbnail.dll'
        Copy-Item -LiteralPath $provider -Destination $nativeProvider
        $provider = $nativeProvider
    }
    $generated = [string]::IsNullOrWhiteSpace($MediaPath)
    if ($generated) {
        $source = Join-Path $temporaryDirectory 'alpha.png'
        [VisualMediaViewerShellSmoke]::CreateAlphaPng($source)
    } else {
        $source = (Resolve-Path -LiteralPath $MediaPath).ProviderPath
    }
    $malformed = Join-Path $temporaryDirectory 'malformed.bin'
    [System.IO.File]::WriteAllBytes($malformed, [byte[]] (0, 1, 2, 3, 4, 5, 6, 7))
    [VisualMediaViewerShellSmoke]::Run($provider, $source, $malformed, $generated)
    if ($NativeImagesOnly -and $generated) {
        $jpeg = Join-Path $temporaryDirectory 'native.jpg'
        [VisualMediaViewerShellSmoke]::CreateJpeg($jpeg)
        [VisualMediaViewerShellSmoke]::Run($provider, $jpeg, $malformed, $false)
        'PNG and JPEG succeeded without any worker EXE beside the thumbnail DLL.'
    }
} finally {
    Get-ChildItem -LiteralPath $temporaryDirectory -File | Remove-Item -Force
    Remove-Item -LiteralPath $temporaryDirectory -Force
}
