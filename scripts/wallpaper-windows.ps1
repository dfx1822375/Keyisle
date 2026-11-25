param([Parameter(Mandatory=$true)][string]$ExePath,[Parameter(Mandatory=$true)][string]$DataDir)
. (Join-Path $PSScriptRoot 'windows-test-helpers.ps1')
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class WallpaperTestNative {
 [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr value);
 [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr window);
 [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
 [DllImport("user32.dll")] public static extern void mouse_event(uint flags,uint x,uint y,uint data,UIntPtr extra);
}
'@
[WallpaperTestNative]::SetThreadDpiAwarenessContext([IntPtr](-4))|Out-Null
$name=[Windows.Automation.AutomationElement]::NameProperty
$master='GUI-Master-Only!2026'
function Pause([int]$ms=300) {
 $watch=[Diagnostics.Stopwatch]::StartNew()
 while ($watch.ElapsedMilliseconds -lt $ms) {
  [Windows.Forms.Application]::DoEvents()
  Start-Sleep -Milliseconds 15
 }
}
function HasMessage([string]$fragment) {
 $type=[Windows.Automation.AutomationElement]::ControlTypeProperty
 return @(Visible (Main) $type ([Windows.Automation.ControlType]::Text) |
     Where-Object {$_.Current.Name.Contains($fragment)}).Count -gt 0
}
function Choose([string]$path) {
 Button '选择背景图片' 100;DialogFile '选择背景图片' $path
 for($i=0;$i -lt 50 -and (TextExists '正在处理图片…');$i++){Pause 100}
 Assert (!(TextExists '正在处理图片…')) 'Wallpaper import did not finish'
}
function Appearance([string]$keys) {
 $mode=@(Visible (Main) $name '外观模式' | Where-Object {
     $_.Current.ControlType -eq [Windows.Automation.ControlType]::ComboBox})[0]
 Assert ($null -ne $mode) 'Missing appearance mode picker'
 $mode.SetFocus();[Windows.Forms.SendKeys]::SendWait($keys);Pause 160
}
function Picture([string]$path,$format) {
 $bitmap=[Drawing.Bitmap]::new(720,420)
 $graphics=[Drawing.Graphics]::FromImage($bitmap)
 $gradient=[Drawing.Drawing2D.LinearGradientBrush]::new([Drawing.Rectangle]::new(0,0,720,420),[Drawing.Color]::DarkViolet,[Drawing.Color]::DarkOrange,[single]25)
 try {
  $graphics.SmoothingMode=[Drawing.Drawing2D.SmoothingMode]::AntiAlias
  $graphics.FillRectangle($gradient,0,0,720,420)
  $graphics.FillEllipse([Drawing.Brushes]::Gold,470,40,150,150)
  for ($x=0;$x -lt 720;$x+=24) {
   $brush=if(($x/24)%2 -eq 0){[Drawing.Brushes]::Black}else{[Drawing.Brushes]::White}
   $graphics.FillRectangle($brush,$x,380,24,40)
  }
  $bitmap.Save($path,$format)
 } finally {$gradient.Dispose();$graphics.Dispose();$bitmap.Dispose()}
}
function ScreenPixel([int]$x,[int]$y) {
 $b=[Drawing.Bitmap]::new(1,1);$g=[Drawing.Graphics]::FromImage($b)
 try {
  $g.CopyFromScreen($x,$y,0,0,$b.Size)
  return $b.GetPixel(0,0).ToArgb()
 } finally {$g.Dispose();$b.Dispose()}
}
function Pixel($window,[int]$y) {
 $h=[IntPtr]$window.Current.NativeWindowHandle
 $r=$window.Current.BoundingRectangle
 $scale=[WallpaperTestNative]::GetDpiForWindow($h)/96.0
 if($y -lt 0){$y+=[int]($r.Height/$scale)}
 Assert ([VaultTestNative]::GetForegroundWindow() -eq $h) 'Test window lost the foreground'
 return ScreenPixel ([int]($r.Left+$r.Width/2)) ([int]($r.Top+$y*$scale))
}
function ColorDistance([int]$left,[int]$right) {
 $a=[Drawing.Color]::FromArgb($left);$b=[Drawing.Color]::FromArgb($right)
 return [Math]::Abs([int]$a.R-$b.R)+[Math]::Abs([int]$a.G-$b.G)+[Math]::Abs([int]$a.B-$b.B)
}
function WallpaperSlider {
 $slider=@(Visible (Main) $name '界面透明度' | Where-Object {
     $_.Current.ControlType -eq [Windows.Automation.ControlType]::Slider})[0]
 Assert ($null -ne $slider) 'Missing interface transparency slider'
 return $slider
}
function Transparency([double]$value) {
 $slider=WallpaperSlider
 $slider.GetCurrentPattern([Windows.Automation.RangeValuePattern]::Pattern).SetValue($value)
 Pause 180
 Assert (HasMessage "$([Math]::Round($value))%") 'Background preview did not respond to slider'
}
function PreviewPixel {
 $h=[IntPtr](Main).Current.NativeWindowHandle
 Assert ([VaultTestNative]::GetForegroundWindow() -eq $h) 'Settings lost the foreground'
 $scale=[WallpaperTestNative]::GetDpiForWindow($h)/96.0
 $r=(WallpaperSlider).Current.BoundingRectangle
 return ScreenPixel ([int]($r.Left-106*$scale)) ([int]($r.Top+$r.Height/2))
}
function WallpaperPayloadHash {
 $bytes=[IO.File]::ReadAllBytes($stored)
 $sha=[Security.Cryptography.SHA256]::Create()
 try {return [Convert]::ToBase64String($sha.ComputeHash($bytes,1,$bytes.Length-1))} finally {$sha.Dispose()}
}
function Capture([string]$file,$window=(Main)) {
 $h=[IntPtr]$window.Current.NativeWindowHandle
 Assert ([VaultTestNative]::GetForegroundWindow() -eq $h) 'Test window lost the foreground'
 $r=$window.Current.BoundingRectangle
 $b=[Drawing.Bitmap]::new([int]$r.Width,[int]$r.Height);$g=[Drawing.Graphics]::FromImage($b)
 try {
  $g.CopyFromScreen([int]$r.Left,[int]$r.Top,0,0,$b.Size)
  $b.Save((Join-Path $DataDir $file),[Drawing.Imaging.ImageFormat]::Png)
 } finally {$g.Dispose();$b.Dispose()}
}
function OpenPopup {
 $receiver.Clear();$form.Show();$form.Activate();$receiver.Focus()|Out-Null
 [VaultTestNative]::SetForegroundWindow($form.Handle)|Out-Null;Pause 200
 Assert ([VaultTestNative]::GetForegroundWindow() -eq $form.Handle) 'Unsafe input target during popup test'
 [Windows.Forms.SendKeys]::SendWait('^%p');Pause 600
 $byId=[Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ProcessIdProperty,$script:process.Id)
 $popup=@([Windows.Automation.AutomationElement]::RootElement.FindAll([Windows.Automation.TreeScope]::Children,$byId) |
     Where-Object {$_.Current.Name -eq 'Keyisle · 快速填充' -and !$_.Current.IsOffscreen})[0]
 Assert ($null -ne $popup) 'Quick fill did not open'
 return $popup
}
Assert (!(Test-Path -LiteralPath $DataDir)) 'Use a new isolated profile'
Assert ([VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents') -eq [IntPtr]::Zero) 'Close an existing Keyisle instance first'
New-Item -ItemType Directory -Path $DataDir|Out-Null
$oldDataDir=$env:PASSWORD_MANAGER_DATA_DIR
$cursor=[Windows.Forms.Cursor]::Position
$env:PASSWORD_MANAGER_DATA_DIR=$DataDir
[IO.File]::WriteAllText((Join-Path $DataDir 'appearance.txt'),'light')
$png=Join-Path $DataDir 'sample.png'
$jpeg=Join-Path $DataDir 'sample.jpg'
$invalid=Join-Path $DataDir 'invalid.jpg'
$stored=Join-Path $DataDir 'wallpaper.bin'
Picture $png ([Drawing.Imaging.ImageFormat]::Png)
Picture $jpeg ([Drawing.Imaging.ImageFormat]::Jpeg)
[IO.File]::WriteAllText($invalid,'not an image')
$form=[Windows.Forms.Form]::new();$form.Text='背景测试的示例登录页';$form.Width=520;$form.Height=160
$receiver=[Windows.Forms.TextBox]::new();$receiver.SetBounds(20,20,380,30);$form.Controls.Add($receiver)
try {
 StartApp
 $plainLocked=Pixel (Main) 10
 SetEdit 0 $master;SetEdit 1 $master;Button '创建密码库' 1400
 Button '新增账号';SetEdit 0 '企业邮箱';SetEdit 1 'mail.example';SetEdit 2 'Synthetic-Only!2026';SetEdit 3 '工作';Button '保存账号'
 $plain=Pixel (Main) 10
 $plainList=Pixel (Main) 235
 $plainPopup=Pixel (OpenPopup) 8
 [Windows.Forms.SendKeys]::SendWait('^+m');Pause 450
 Button '设置';Choose $invalid
 Assert ((HasMessage '无法读取图片') -and !(Test-Path -LiteralPath $stored)) 'Invalid image was accepted'
 Choose $jpeg
 Assert (TextExists '移除图片') 'JPEG was not previewed'
 Button '返回账号列表'
 Assert (TextExists '保留当前修改？') 'Wallpaper draft did not require confirmation'
 Button '放弃修改'
 Assert ((Pixel (Main) 10) -eq $plain -and !(Test-Path -LiteralPath $stored)) 'Discarded preview was applied'
 Button '设置';Choose $jpeg;Button '保存设置'
 Assert (([IO.File]::ReadAllBytes($stored))[26] -eq 2) 'JPEG was not stored as RGB PNG'
 Choose $png
 Transparency 0
 $lightPreview=PreviewPixel
 Appearance '{DOWN}'
 Assert ((ColorDistance (PreviewPixel) $lightPreview) -gt 450) 'Wallpaper preview did not follow the unsaved dark appearance'
 Appearance '{UP}'
 Assert ((PreviewPixel) -eq $lightPreview) 'Wallpaper preview did not return to the light appearance'
 Button '保存设置';Button '返回账号列表'
 Assert ((Pixel (Main) 10) -eq $plain -and (Test-Path -LiteralPath $stored)) 'Zero transparency hid or removed the wrong layer'
 $payload=WallpaperPayloadHash
 Button '设置';Transparency 35;Button '保存设置';Button '返回账号列表'
 Assert ((WallpaperPayloadHash) -eq $payload) 'Updating transparency rewrote the PNG payload'
 $low=Pixel (Main) 10
 $lowList=Pixel (Main) 235
 Button '设置';Transparency 35.2;Button '返回账号列表'
 Assert (!(TextExists '保留当前修改？')) 'An unchanged rounded transparency opened the discard prompt'
 Button '设置';Transparency 90
 Capture 'wallpaper-settings.png'
 Button '保存设置';Button '返回账号列表'
 Assert (([IO.File]::ReadAllBytes($stored))[0] -eq (0x80 -bor 90)) 'Transparency preference was not saved'
 $colored=Pixel (Main) 10
 $coloredList=Pixel (Main) 235
 Assert ((ColorDistance $colored $plain) -gt 1.7*(ColorDistance $low $plain)) 'High transparency did not reveal noticeably more image'
 Assert ((ColorDistance $coloredList $plainList) -gt 2*(ColorDistance $lowList $plainList)) 'The list surface did not become transparent'
 Assert ((ColorDistance (Pixel (Main) -28) ([Drawing.Color]::White.ToArgb())) -le 75) 'The light footer did not protect its shortcut text'
 Capture 'wallpaper-main.png'
 SetEdit 0 'youxiang'
 Assert (TextExists '企业邮箱') 'The translucent search field did not update the list'
 Button '清除搜索'
 Assert (!(TextExists '清除筛选')) 'The translucent clear button did not reset the query'
 $category=@(Visible (Main) $name '分类筛选')[0]
 Assert ($null -ne $category) 'Translucent category picker lost accessibility'
 $search=@(Visible (Main) $name '搜索账号')[0]
 $edge=$search.Current.BoundingRectangle
 $mainHandle=[IntPtr](Main).Current.NativeWindowHandle
 Assert ([VaultTestNative]::GetForegroundWindow() -eq $mainHandle) 'Unsafe mouse target during search test'
 foreach($x in @(($edge.Left-5),($edge.Right+5))) {
  $category.SetFocus();Pause 100
  Assert (!$search.Current.HasKeyboardFocus) 'Could not move focus away from search'
  [WallpaperTestNative]::SetCursorPos([int]$x,[int]($edge.Top+$edge.Height/2))|Out-Null
  [WallpaperTestNative]::mouse_event(2,0,0,0,[UIntPtr]::Zero)
  [WallpaperTestNative]::mouse_event(4,0,0,0,[UIntPtr]::Zero);Pause 150
  Assert ($search.Current.HasKeyboardFocus) 'Search padding did not focus the input'
 }
 $category.SetFocus();[Windows.Forms.SendKeys]::SendWait('{ENTER}');Pause 240
 Capture 'wallpaper-filter.png'
 [Windows.Forms.SendKeys]::SendWait('{ESC}');Pause 170
 $filter=$category.Current.BoundingRectangle
 [WallpaperTestNative]::SetCursorPos([int]($filter.Right-12),[int]($filter.Top+$filter.Height/2))|Out-Null;Pause 180
 [WallpaperTestNative]::mouse_event(2,0,0,0,[UIntPtr]::Zero)
 [WallpaperTestNative]::mouse_event(4,0,0,0,[UIntPtr]::Zero);Pause 180
 $expanded=$category.GetCurrentPattern([Windows.Automation.ExpandCollapsePattern]::Pattern)
 Assert ($expanded.Current.ExpandCollapseState -eq [Windows.Automation.ExpandCollapseState]::Expanded) 'Click did not expand the translucent filter'
 [Windows.Forms.SendKeys]::SendWait('{ESC}');Pause 150
 $category.SetFocus();[Windows.Forms.SendKeys]::SendWait('{DOWN}');Pause 170
 Assert ((TextExists '企业邮箱') -and (HasMessage '工作')) 'Translucent category picker did not respond to the keyboard'
 Button '清除筛选'
 Button '收起'
 $pop=OpenPopup
 Assert ((Pixel $pop 8) -ne $plainPopup) 'Saved background did not render in quick fill'
 Capture 'wallpaper-quick-fill.png' $pop
 $search=@(Visible $pop $name '搜索账号')[0]
 $search.GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).SetValue('youxiang')
 $search.SetFocus();Pause 180
 Assert (@(Visible $pop ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Button) |
         Where-Object {$_.Current.Name.StartsWith('已选择，企业邮箱')}).Count -eq 1) 'The translucent popup query did not select the credential'
 Assert ([VaultTestNative]::GetForegroundWindow() -eq [IntPtr]$pop.Current.NativeWindowHandle) 'Quick fill lost the foreground'
 SendTestKeys '{ENTER}'
 for ($i=0;$i -lt 30 -and $receiver.Text -ne 'Synthetic-Only!2026';$i++) {Pause 100}
 if ($receiver.Text -ne 'Synthetic-Only!2026' -and [VaultTestNative]::GetForegroundWindow() -eq [IntPtr]$pop.Current.NativeWindowHandle) {
  @(Visible $pop ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Text)) |
      ForEach-Object {Write-Output $_.Current.Name}
 }
 Assert ($receiver.Text -eq 'Synthetic-Only!2026') 'The translucent popup search could not fill the synthetic credential'
 $pop=OpenPopup
 [Windows.Forms.SendKeys]::SendWait('^+m');Pause 450
 Remove-Item -LiteralPath $png
 Button '锁定'
 $locked=Pixel (Main) 10
 Assert ($locked -eq $plainLocked) 'The lock screen displayed the wallpaper'
 StopApp
 $exclusive=[IO.File]::Open($stored,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::None)
 try {
  StartApp
  Assert ((Pixel (Main) 10) -eq $locked) 'Locked window appearance changed on restart'
 } finally {$exclusive.Dispose()}
 Unlock $master
 Assert ((Pixel (Main) 10) -eq $colored) 'The managed copy did not survive restart or removal of the source'
 Button '设置'
 Appearance '{DOWN}'
 $exclusive=[IO.File]::Open($stored,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::None)
 try {Button '保存设置';Button '返回账号列表'} finally {$exclusive.Dispose()}
 $darkColored=Pixel (Main) 10
 Assert (([IO.File]::ReadAllText((Join-Path $DataDir 'appearance.txt'))).Trim() -eq 'dark' -and
     $darkColored -ne $colored -and (([IO.File]::ReadAllBytes($stored))[0] -eq (0x80 -bor 90))) 'Dark mode did not preserve the wallpaper'
 Assert ((ColorDistance (Pixel (Main) -28) ([Drawing.Color]::FromArgb(28,36,48).ToArgb())) -le 75) 'The dark footer did not protect its shortcut text'
 Capture 'wallpaper-dark.png'
 StopApp
 $legacy=[IO.File]::Open($stored,[IO.FileMode]::Open,[IO.FileAccess]::Write)
 try {$legacy.WriteByte(35)}finally{$legacy.Dispose()}
 StartApp;Unlock $master
 Assert ((Pixel (Main) 10) -eq $darkColored) 'The legacy maximum did not migrate to the new maximum'
 Button '设置'
 Assert (HasMessage '90%') 'The legacy transparency position was not restored'
 Appearance '{UP}'
 Button '移除图片';Button '保存设置';Button '返回账号列表'
 Assert (!(Test-Path -LiteralPath $stored) -and (Pixel (Main) 10) -eq $plain) 'Removing the saved wallpaper did not restore the canvas'
 Write-Output 'PASS: background draft, lazy load, RGB import, selective settings save, search padding and fill, contrast, migration, and removal.'
} finally {
 StopApp
 $form.Dispose()
 [WallpaperTestNative]::SetCursorPos($cursor.X,$cursor.Y)|Out-Null
 $env:PASSWORD_MANAGER_DATA_DIR=$oldDataDir
}
