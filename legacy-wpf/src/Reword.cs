using System;
using System.IO;
using System.Diagnostics;
using System.Globalization;
using System.Runtime.InteropServices;
using System.Threading;
using System.Windows;
using System.Windows.Media;
using System.Windows.Media.Imaging;
using System.Windows.Interop;
using System.Windows.Threading;
using Forms = System.Windows.Forms;
using Drawing = System.Drawing;

namespace Reword {
    internal static class Native {
        [DllImport("user32.dll")] internal static extern bool RegisterHotKey(IntPtr hwnd, int id, uint modifiers, uint key);
        [DllImport("user32.dll")] internal static extern bool UnregisterHotKey(IntPtr hwnd, int id);
        [DllImport("user32.dll")] internal static extern short GetAsyncKeyState(int key);
        [DllImport("user32.dll")] internal static extern int GetWindowLong(IntPtr hwnd, int index);
        [DllImport("user32.dll")] internal static extern int SetWindowLong(IntPtr hwnd, int index, int value);
        [DllImport("user32.dll")] internal static extern bool SetProcessDPIAware();
        internal static bool Held() { return Down(0x11) && Down(0x12) && Down(0x48); }
        private static bool Down(int key) { return (GetAsyncKeyState(key) & 0x8000) != 0; }
    }

    internal sealed class Palette {
        internal Color Background = Parse("#202C3A");
        internal Color Accent = Parse("#FFB482");
        internal static string FileName { get { return Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "colors.ini"); } }
        internal static Color Parse(string value) { return (Color)ColorConverter.ConvertFromString(value); }
        internal void Load() {
            if (!File.Exists(FileName)) return;
            foreach (string line in File.ReadAllLines(FileName)) {
                string[] pair = line.Split('=');
                if (pair.Length != 2) continue;
                try {
                    if (pair[0].Trim() == "background") Background = Parse(pair[1].Trim());
                    if (pair[0].Trim() == "accent") Accent = Parse(pair[1].Trim());
                } catch (FormatException) { } catch (NotSupportedException) { }
            }
        }
        internal void Save() {
            File.WriteAllText(FileName, "background=" + Background.ToString() + "\r\naccent=" + Accent.ToString() + "\r\n");
        }
    }

    // A single drawing surface keeps the capsule, icon and stream on the same clock.
    internal sealed class Capsule : FrameworkElement {
        internal double Time;
        internal double Exit = 1;
        internal readonly Palette Palette;
        private readonly Geometry micBody = Geometry.Parse("M12,19 L12,22 M19,10 L19,12 A7,7 0 0 1 5,12 L5,10");
        internal Capsule(Palette palette) { Palette = palette; Width = 224; Height = 90; }
        internal static double Clamp(double x) { return Math.Max(0, Math.Min(1, x)); }
        // Flat anticipation, a quick middle, and a soft arrival, matching the sketch.
        internal static double Ease(double x) { x = Clamp(x); return x*x*x*(x*(x*6-15)+10); }
        private static SolidColorBrush Brush(Color c) { SolidColorBrush b = new SolidColorBrush(c); b.Freeze(); return b; }
        protected override void OnRender(DrawingContext dc) {
            base.OnRender(dc);
            double expansion = Ease(Time / .5);
            double enter = Ease(Time / .16);
            double width = (22 + 170 * expansion) * (.88 + .12 * Exit);
            double height = 7 + 47 * Ease(Time / .34);
            double cx = 112, cy = 45 + (1-Exit)*5;
            Rect bounds = new Rect(cx-width/2, cy-height/2, width, height);
            dc.PushOpacity(enter * Exit);
            // Small, soft shadow without a rectangular window backdrop.
            for (int i = 7; i > 0; i--) {
                dc.DrawRoundedRectangle(Brush(Color.FromArgb((byte)(i == 1 ? 15 : 4), 0, 0, 0)), null,
                    new Rect(bounds.X-i, bounds.Y+3-i, bounds.Width+2*i, bounds.Height+2*i), height/2+i, height/2+i);
            }
            dc.DrawRoundedRectangle(Brush(Palette.Background), new Pen(Brush(Color.FromArgb(24,255,255,255)), 1), bounds, height/2, height/2);
            dc.PushClip(new RectangleGeometry(bounds, height/2, height/2));

            double micX = cx - 68 * Ease((Time-.035)/.465);
            double travelSpeed = 187;
            // Bars are born at the moving microphone and flow right. The front reaches
            // the capsule edge at about 750 ms, after the shell has settled at 500 ms.
            double firstBirth = .12;
            int last = (int)Math.Floor((Time-firstBirth)/.047);
            for (int i = Math.Max(0, last-28); i <= last; i++) {
                double born = firstBirth + i*.047;
                double age = Time-born;
                if (age < 0) continue;
                double origin = cx - 68*Ease((born-.035)/.465) + 18;
                double x = origin + age*travelSpeed;
                double leftLimit = micX+20;
                double rightLimit = 132 + 59*Ease((Time-.12)/.63);
                if (x < leftLimit || x > rightLimit+7) continue;
                double envelope = Ease(age/.095) * Clamp((rightLimit+7-x)/14);
                double seed = .5 + .5*Math.Sin(i*2.39996 + .8);
                double pulse = .78 + .22*Math.Sin(Time*7.5-i*.74);
                double barHeight = (5 + 22*seed*seed)*pulse*envelope;
                Pen bar = new Pen(Brush(Palette.Accent), 2.6);
                bar.StartLineCap = PenLineCap.Round; bar.EndLineCap = PenLineCap.Round;
                dc.PushOpacity(.9*envelope);
                dc.DrawLine(bar, new Point(x,cy-barHeight/2), new Point(x,cy+barHeight/2));
                dc.Pop();
            }

            double iconShow = Ease((Time-.035)/.19);
            dc.PushOpacity(iconShow);
            double iconScale = .72 + .28*Ease((Time-.035)/.3);
            dc.PushTransform(new TranslateTransform(micX-12*iconScale, cy-12*iconScale));
            dc.PushTransform(new ScaleTransform(iconScale,iconScale));
            Pen iconPen = new Pen(Brush(Palette.Accent), 1.9);
            iconPen.StartLineCap = PenLineCap.Round; iconPen.EndLineCap = PenLineCap.Round; iconPen.LineJoin = PenLineJoin.Round;
            // Lucide mic.svg, retained in assets with the original license.
            dc.DrawGeometry(null, iconPen, micBody);
            dc.DrawRoundedRectangle(null, iconPen, new Rect(9,2,6,13), 3,3);
            dc.Pop(); dc.Pop(); dc.Pop(); dc.Pop(); dc.Pop();
        }
    }

    internal sealed class Overlay : Window {
        internal readonly Capsule Surface;
        private readonly Stopwatch clock = new Stopwatch();
        private readonly DispatcherTimer releaseTimer = new DispatcherTimer();
        private bool closing, preview;
        private double closeAt;
        private IntPtr handle;
        internal Overlay(Palette palette) {
            Title = "Reword"; Width = 224; Height = 90;
            WindowStyle = WindowStyle.None; ResizeMode = ResizeMode.NoResize;
            AllowsTransparency = true; Background = Brushes.Transparent;
            Topmost = true; ShowInTaskbar = false; ShowActivated = false;
            Surface = new Capsule(palette); Content = Surface;
            SourceInitialized += delegate {
                handle = new WindowInteropHelper(this).Handle;
                Native.SetWindowLong(handle, -20, Native.GetWindowLong(handle,-20) | 0x08000000 | 0x80);
                HwndSource.FromHwnd(handle).AddHook(HotKey);
                if (!Native.RegisterHotKey(handle, 1, 0x4003, 0x48)) {
                    Forms.MessageBox.Show("Ctrl+Alt+H is already used by another app. Close that app and restart Reword.", "Reword");
                }
            };
            Closed += delegate { Native.UnregisterHotKey(handle,1); Stop(); };
            releaseTimer.Interval = TimeSpan.FromMilliseconds(12);
            releaseTimer.Tick += delegate {
                if (!preview && !closing && !Native.Held() && clock.Elapsed.TotalSeconds > .045) BeginClose();
            };
            MouseRightButtonUp += delegate { RewordApp.Instance.ShowMenu(); };
        }
        private IntPtr HotKey(IntPtr hwnd,int msg,IntPtr w,IntPtr l,ref bool handled) {
            if (msg == 0x312 && w.ToInt32() == 1) { Open(false); handled = true; }
            return IntPtr.Zero;
        }
        internal void InitializeHidden() { Show(); Hide(); }
        internal void Open(bool isPreview) {
            preview = isPreview; closing = false;
            Rect area = SystemParameters.WorkArea;
            Left = area.Left+(area.Width-Width)/2;
            Top = area.Bottom-Height-22;
            Surface.Time = 0; Surface.Exit = 1;
            Surface.InvalidateVisual();
            clock.Restart();
            CompositionTarget.Rendering -= Frame;
            CompositionTarget.Rendering += Frame;
            Show(); releaseTimer.Start();
        }
        private void Frame(object sender, EventArgs args) {
            double now = clock.Elapsed.TotalSeconds;
            Surface.Time = now;
            if (preview && now > 5 && !closing) BeginClose();
            if (closing) {
                Surface.Exit = 1-Capsule.Ease((now-closeAt)/.22);
                if (now-closeAt >= .22) { Hide(); Stop(); return; }
            }
            Surface.InvalidateVisual();
        }
        private void BeginClose() { closing = true; closeAt = clock.Elapsed.TotalSeconds; }
        private void Stop() { releaseTimer.Stop(); CompositionTarget.Rendering -= Frame; clock.Stop(); }
    }

    internal sealed class RewordApp : Application {
        internal static RewordApp Instance;
        private Forms.NotifyIcon tray;
        private Forms.ContextMenuStrip menu;
        private Drawing.Icon trayIcon;
        private readonly Palette palette = new Palette();
        private Overlay overlay;
        private Forms.Form settings;
        internal RewordApp() { Instance = this; ShutdownMode = ShutdownMode.OnExplicitShutdown; }
        protected override void OnStartup(StartupEventArgs e) {
            base.OnStartup(e); palette.Load();
            if (Array.IndexOf(e.Args,"--render-frames") >= 0) { RenderFrames(); Shutdown(); return; }
            overlay = new Overlay(palette); MainWindow = overlay; overlay.InitializeHidden();
            menu = new Forms.ContextMenuStrip();
            menu.Items.Add("Preview animation", null, delegate { overlay.Open(true); });
            menu.Items.Add("Colors...", null, delegate { Settings(); });
            menu.Items.Add(new Forms.ToolStripSeparator());
            menu.Items.Add("Exit Reword", null, delegate { Shutdown(); });
            trayIcon = MakeIcon();
            tray = new Forms.NotifyIcon(); tray.Text = "Reword | Hold Ctrl+Alt+H";
            tray.Icon = trayIcon; tray.ContextMenuStrip = menu; tray.Visible = true;
            tray.DoubleClick += delegate { Settings(); };
            if (Array.IndexOf(e.Args,"--preview") >= 0) overlay.Open(true);
            if (Array.IndexOf(e.Args,"--settings") >= 0) Settings();
        }
        internal void ShowMenu() { menu.Show(Forms.Cursor.Position); }
        private Drawing.Icon MakeIcon() {
            using (Drawing.Bitmap bitmap = new Drawing.Bitmap(32,32)) {
                using (Drawing.Graphics g = Drawing.Graphics.FromImage(bitmap)) {
                    g.SmoothingMode = Drawing.Drawing2D.SmoothingMode.AntiAlias;
                    g.Clear(Drawing.Color.Transparent);
                    using (Drawing.Brush bg = new Drawing.SolidBrush(Drawing.Color.FromArgb(palette.Background.R,palette.Background.G,palette.Background.B))) g.FillEllipse(bg,1,1,30,30);
                    using (Drawing.Pen pen = new Drawing.Pen(Drawing.Color.FromArgb(palette.Accent.R,palette.Accent.G,palette.Accent.B),1.9f)) {
                        pen.StartCap = Drawing.Drawing2D.LineCap.Round; pen.EndCap = Drawing.Drawing2D.LineCap.Round;
                        g.DrawArc(pen,12,6,8,8,180,180); g.DrawLine(pen,12,10,12,16); g.DrawLine(pen,20,10,20,16);
                        g.DrawArc(pen,12,12,8,8,0,180); g.DrawArc(pen,8,10,16,15,0,180); g.DrawLine(pen,8,16,8,17); g.DrawLine(pen,24,16,24,17); g.DrawLine(pen,16,25,16,28);
                    }
                }
                IntPtr icon = bitmap.GetHicon();
                try { return (Drawing.Icon)Drawing.Icon.FromHandle(icon).Clone(); } finally { DestroyIcon(icon); }
            }
        }
        [DllImport("user32.dll")] private static extern bool DestroyIcon(IntPtr h);
        private void RefreshIcon() { Drawing.Icon old = trayIcon; trayIcon = MakeIcon(); tray.Icon = trayIcon; old.Dispose(); }
        private void Pick(bool background, Forms.Button button) {
            using (Forms.ColorDialog dialog = new Forms.ColorDialog()) {
                Color color = background ? palette.Background : palette.Accent;
                dialog.Color = Drawing.Color.FromArgb(color.R,color.G,color.B); dialog.FullOpen = true;
                if (dialog.ShowDialog(settings) != Forms.DialogResult.OK) return;
                Color selected = Color.FromRgb(dialog.Color.R,dialog.Color.G,dialog.Color.B);
                if (background) palette.Background = selected; else palette.Accent = selected;
                palette.Save(); RefreshIcon(); button.BackColor = dialog.Color; overlay.Open(true);
            }
        }
        private void Settings() {
            if (settings != null && !settings.IsDisposed) { settings.Activate(); return; }
            settings = new Forms.Form(); settings.Text = "Reword"; settings.ClientSize = new Drawing.Size(350,260);
            settings.FormBorderStyle = Forms.FormBorderStyle.FixedDialog; settings.MaximizeBox = false;
            settings.StartPosition = Forms.FormStartPosition.CenterScreen;
            settings.BackColor = Drawing.Color.FromArgb(25,33,44); settings.ForeColor = Drawing.Color.FromArgb(233,237,242);
            settings.Font = new Drawing.Font("Segoe UI",10);
            Forms.Label heading = new Forms.Label(); heading.Text = "Hold Ctrl + Alt + H"; heading.Font = new Drawing.Font("Segoe UI",15); heading.SetBounds(24,22,310,32); settings.Controls.Add(heading);
            Forms.Label note = new Forms.Label(); note.Text = "Appearance preview. Voice input comes later."; note.SetBounds(24,63,310,42); settings.Controls.Add(note);
            AddColorButton("Background",true,112); AddColorButton("Microphone and waves",false,155);
            Forms.Button preview = new Forms.Button(); preview.Text = "Replay animation"; preview.SetBounds(24,209,302,32);
            preview.FlatStyle = Forms.FlatStyle.Flat; preview.Click += delegate { overlay.Open(true); }; settings.Controls.Add(preview);
            settings.Show();
        }
        private void AddColorButton(string text,bool background,int y) {
            Forms.Label label = new Forms.Label(); label.Text = text; label.SetBounds(24,y+5,220,26); settings.Controls.Add(label);
            Color c = background ? palette.Background : palette.Accent;
            Forms.Button button = new Forms.Button(); button.SetBounds(268,y,58,30);
            button.BackColor = Drawing.Color.FromArgb(c.R,c.G,c.B); button.FlatStyle = Forms.FlatStyle.Flat;
            button.AccessibleName = text; button.Click += delegate { Pick(background,button); }; settings.Controls.Add(button);
        }
        private void RenderFrames() {
            string folder = Path.Combine(AppDomain.CurrentDomain.BaseDirectory,"preview"); Directory.CreateDirectory(folder);
            string assets = Path.Combine(AppDomain.CurrentDomain.BaseDirectory,"assets"); Directory.CreateDirectory(assets);
            using (Drawing.Icon icon = MakeIcon())
            using (FileStream output = File.Create(Path.Combine(assets,"Reword.ico"))) icon.Save(output);
            foreach (int ms in new int[] {0,75,125,200,250,375,500,625,750,1500}) {
                Capsule surface = new Capsule(palette); surface.Time = ms/1000.0;
                surface.Measure(new Size(224,90)); surface.Arrange(new Rect(0,0,224,90)); surface.UpdateLayout();
                RenderTargetBitmap image = new RenderTargetBitmap(448,180,192,192,PixelFormats.Pbgra32); image.Render(surface);
                PngBitmapEncoder encoder = new PngBitmapEncoder(); encoder.Frames.Add(BitmapFrame.Create(image));
                using (FileStream file = File.Create(Path.Combine(folder,ms.ToString("D4")+".png"))) encoder.Save(file);
            }
        }
        protected override void OnExit(ExitEventArgs e) {
            if (tray != null) { tray.Visible = false; tray.Dispose(); }
            if (trayIcon != null) trayIcon.Dispose();
            if (settings != null) settings.Dispose();
            base.OnExit(e);
        }
        [STAThread] public static void Main(string[] args) {
            Native.SetProcessDPIAware();
            bool created;
            using (Mutex single = new Mutex(true,"Local\\Reword.Appearance",out created)) {
                if (!created && Array.IndexOf(args,"--render-frames") < 0) return;
                try { new RewordApp().Run(); }
                catch (Exception error) { File.WriteAllText(Path.Combine(AppDomain.CurrentDomain.BaseDirectory,"error.log"),error.ToString()); Forms.MessageBox.Show(error.Message,"Reword"); }
            }
        }
    }
}
