param([string]$ExePath,[string]$DataDir)
. (Join-Path $PSScriptRoot 'windows-test-helpers.ps1')
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;using System.Runtime.InteropServices;
public static class DensityNative {
 [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
 [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h,IntPtr after,int x,int y,int width,int height,uint flags);
}
'@
function Capture([string]$name){$r=(Main).Current.BoundingRectangle;$b=[Drawing.Bitmap]::new([int]$r.Width,[int]$r.Height);$g=[Drawing.Graphics]::FromImage($b);try{$g.CopyFromScreen([int]$r.X,[int]$r.Y,0,0,$b.Size);$b.Save((Join-Path $DataDir $name),[Drawing.Imaging.ImageFormat]::Png)}finally{$g.Dispose();$b.Dispose()}}
$env:PASSWORD_MANAGER_DATA_DIR=$DataDir;$script:process=$null
try{
 StartApp;Unlock
 $title='多账号长名称-'+('LongName测量'*12)
 foreach($user in @('account.one@example.invalid','account.two@example.invalid')){
  Button '新增账号';SetEdit 0 $title;SetEdit 1 $user;SetEdit 2 'Density-Only!2026';SetEdit 3 '工作';SetEdit 4 ('边界标记,'+((1..24|ForEach-Object{"标签项目$_"}) -join ','));SetEdit 5 ('https://example.invalid/'+('long-path/'*16));SetEdit 6 ('多行备注测试。'*120);Button '保存账号'
 }
 $h=[IntPtr](Main).Current.NativeWindowHandle;$scale=[DensityNative]::GetDpiForWindow($h)/96.0
 [DensityNative]::SetWindowPos($h,[IntPtr]::Zero,0,0,[int](640*$scale),[int](600*$scale),4)|Out-Null;Pause 500
 SetEdit 0 '边界标记';Capture 'density-list-640.png'
 $rows=@(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Button)|Where-Object{$_.Current.Name.Contains($title)})
 Assert ($rows.Count -eq 2) 'Long-name multi-account search lost results'
 Assert (@($rows|Where-Object{$_.Current.Name.Contains('account.one@example.invalid')}).Count -eq 1) 'First account identity lost'
 Assert (@($rows|Where-Object{$_.Current.Name.Contains('account.two@example.invalid')}).Count -eq 1) 'Second account identity lost'
 Button '编辑账号';Capture 'density-editor-640.png'
 $window=(Main).Current.BoundingRectangle;$save=@(Visible (Main) ([Windows.Automation.AutomationElement]::NameProperty) '保存账号')[0].Current.BoundingRectangle
 Assert ($save.Height -gt 0 -and $save.Bottom -le $window.Bottom) 'Editor action covered in small window'
 Button '取消编辑';Button '设置';Capture 'density-settings-640.png'
 $save=@(Visible (Main) ([Windows.Automation.AutomationElement]::NameProperty) '保存设置')[0].Current.BoundingRectangle
 Assert ($save.Bottom -le $window.Bottom) 'Settings save covered in small window'
 $combo=@(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::ComboBox))[0];$combo.SetFocus();[Windows.Forms.SendKeys]::SendWait(' ');Pause 200;Capture 'density-combo-open.png';[Windows.Forms.SendKeys]::SendWait('{ESC}');Pause 100
 Write-Output 'PASS: long mixed-script names, duplicate names with distinct users, many tags and long notes retain identity; editor and settings actions stay visible at 640x600; dropdown inspected.'
}finally{StopApp}
