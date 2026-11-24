param([string]$ExePath,[string]$DataDir)
. (Join-Path $PSScriptRoot 'windows-test-helpers.ps1')
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System;using System.Runtime.InteropServices;
[StructLayout(LayoutKind.Sequential)] public struct FrameRect { public int Left,Top,Right,Bottom; }
[StructLayout(LayoutKind.Sequential)] public struct FramePoint { public int X,Y; }
public static class FrameNative {
 [DllImport("user32.dll")] public static extern bool AllowSetForegroundWindow(uint process);
 [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(FramePoint point);
 [DllImport("user32.dll")] public static extern IntPtr SetThreadDpiAwarenessContext(IntPtr context);
 [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h,out FrameRect r);
 [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h,uint m,IntPtr w,IntPtr l);
 [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h,ref FramePoint p);
 [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
 [DllImport("user32.dll",EntryPoint="GetWindowLongPtrW")] public static extern IntPtr GetStyle(IntPtr h,int index);
 [DllImport("user32.dll")] public static extern int GetWindowRgn(IntPtr h,IntPtr r);
 [DllImport("gdi32.dll")] public static extern IntPtr CreateRectRgn(int a,int b,int c,int d);
 [DllImport("gdi32.dll")] public static extern bool PtInRegion(IntPtr r,int x,int y);
 [DllImport("gdi32.dll")] public static extern bool DeleteObject(IntPtr r);
 [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
 [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr h);
 [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
 [DllImport("user32.dll")] public static extern bool SetCursorPos(int x,int y);
 [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h,IntPtr after,int x,int y,int width,int height,uint flags);
 [DllImport("user32.dll")] public static extern void mouse_event(uint f,uint x,uint y,uint data,UIntPtr extra);
 [DllImport("user32.dll")] public static extern uint MapVirtualKey(uint k,uint type);
}
'@
[FrameNative]::SetThreadDpiAwarenessContext([IntPtr](-4))|Out-Null
function Capture([string]$name,$root=(Main)){$r=$root.Current.BoundingRectangle;$b=[Drawing.Bitmap]::new([int]$r.Width,[int]$r.Height);$g=[Drawing.Graphics]::FromImage($b);try{$g.CopyFromScreen([int]$r.X,[int]$r.Y,0,0,$b.Size);$b.Save((Join-Path $DataDir $name),[Drawing.Imaging.ImageFormat]::Png)}finally{$g.Dispose();$b.Dispose()}}
function Rounded([IntPtr]$h){$r=[FrameNative]::CreateRectRgn(0,0,1,1);try{return [FrameNative]::GetWindowRgn($h,$r) -eq 3 -and -not [FrameNative]::PtInRegion($r,0,0) -and [FrameNative]::PtInRegion($r,16,16)}finally{[FrameNative]::DeleteObject($r)|Out-Null}}
function Manage{
 $events=[VaultTestNative]::FindWindow('PasswordManagerPrototypeEvents','PasswordManagerPrototypeEvents')
 [uint32]$owner=0;$thread=[VaultTestNative]::GetWindowThreadProcessId($events,[ref]$owner)
 Assert ($owner -eq $script:process.Id) 'Unexpected tray window owner'
 [FrameNative]::AllowSetForegroundWindow($script:process.Id)|Out-Null
 $sent=[VaultTestNative]::PostMessage($events,0x8001,[IntPtr]::Zero,[IntPtr]0x202)
 Write-Output "MANAGE event=$events owner=$owner expected=$($script:process.Id) sent=$sent"
 Pause 1500
 Write-Output ("RAW visible="+[FrameNative]::IsWindowVisible($h)+" iconic="+[FrameNative]::IsIconic($h))
 for($retry=0;$retry -lt 30;$retry++){try{$root=Main;break}catch{Pause 100}}
 $current=[IntPtr](Main).Current.NativeWindowHandle
 Write-Output "MANAGE current=$current iconic=$([FrameNative]::IsIconic($current))"
}

function Popup{$filter=[Windows.Automation.AndCondition]::new([Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::ProcessIdProperty,$script:process.Id),[Windows.Automation.PropertyCondition]::new([Windows.Automation.AutomationElement]::NameProperty,'Keyisle · 快速填充'));return [Windows.Automation.AutomationElement]::RootElement.FindFirst([Windows.Automation.TreeScope]::Children,$filter)}
function OpenPopup{
 $form.Show();$form.Activate();$receiverBox.Focus()|Out-Null;[VaultTestNative]::SetForegroundWindow($form.Handle)|Out-Null;Pause 200
 foreach($key in @(0x11,0x12,0x50)){[VaultTestNative]::keybd_event($key,[byte][FrameNative]::MapVirtualKey($key,0),0,[UIntPtr]::Zero);Pause 60}
 foreach($key in @(0x50,0x12,0x11)){[VaultTestNative]::keybd_event($key,[byte][FrameNative]::MapVirtualKey($key,0),2,[UIntPtr]::Zero);Pause 60}
 for($i=0;$i -lt 25 -and $null -eq (Popup);$i++){Pause 100}
 Assert ($null -ne (Popup)) 'Popup did not open'
}
function ToMain{
 foreach($key in @(0x11,0x10,0x4d)){[VaultTestNative]::keybd_event($key,[byte][FrameNative]::MapVirtualKey($key,0),0,[UIntPtr]::Zero);Pause 60}
 foreach($key in @(0x4d,0x10,0x11)){[VaultTestNative]::keybd_event($key,[byte][FrameNative]::MapVirtualKey($key,0),2,[UIntPtr]::Zero);Pause 60}
 for($i=0;$i -lt 30;$i++){try{$null=Main;return}catch{Pause 100}}
 throw 'Management shortcut did not open main'
}
trap {Write-Output $_.Exception.Message;Write-Output $_.ScriptStackTrace;try{Capture 'failure.png'}catch{};throw}
Assert (-not (Test-Path (Join-Path $DataDir 'vault.db'))) 'Frame test requires empty isolated directory'
$env:PASSWORD_MANAGER_DATA_DIR=$DataDir;New-Item -ItemType Directory -Force $DataDir|Out-Null
$script:process=$null;$form=[Windows.Forms.Form]::new();$form.Text='Frame test receiver';$receiverBox=[Windows.Forms.TextBox]::new();$form.Controls.Add($receiverBox)
try{
 StartApp;Pause 500;$h=[IntPtr](Main).Current.NativeWindowHandle;$r=(Main).Current.BoundingRectangle;$scale=[FrameNative]::GetDpiForWindow($h)/96.0
 Assert (($r.Width/$scale) -le 480 -and ($r.Height/$scale) -le 330) 'Create page still sits in a large outer window'
 $nativeRect=[FrameRect]::new();$origin=[FramePoint]::new();[FrameNative]::GetWindowRect($h,[ref]$nativeRect)|Out-Null;[FrameNative]::ClientToScreen($h,[ref]$origin)|Out-Null
 Assert (($origin.Y-$nativeRect.Top) -le 8*$scale) 'System caption still consumes vertical space'
 Assert (Rounded $h) 'Auth window has no rounded clipping';Capture 'frame-create.png'
 for($cycle=0;$cycle -lt 3;$cycle++){
  $minButton=@(Visible (Main) ([Windows.Automation.AutomationElement]::NameProperty) '最小化窗口')[0].Current.BoundingRectangle
  $nativeBounds=[FrameRect]::new();[FrameNative]::GetWindowRect($h,[ref]$nativeBounds)|Out-Null
  Write-Output ("CYCLE $cycle foreground="+[VaultTestNative]::GetForegroundWindow()+" main=$h native="+$nativeBounds.Left+','+$nativeBounds.Top+" button="+$minButton.Left+','+$minButton.Top)
  [VaultTestNative]::SetForegroundWindow($h)|Out-Null;Pause 150
  $mx=[int]($minButton.Left+$minButton.Width/2);$my=[int]($minButton.Top+$minButton.Height/2);$probe=[FramePoint]::new();$probe.X=$mx;$probe.Y=$my;Write-Output ('MIN TARGET='+[FrameNative]::WindowFromPoint($probe));Write-Output ('MIN HIT='+[FrameNative]::SendMessage($h,0x84,[IntPtr]::Zero,[IntPtr](($mx -band 65535) -bor (($my -band 65535) -shl 16))))
  [FrameNative]::SetCursorPos($mx-80,$my+80)|Out-Null;Pause 100
  if($cycle -eq 1){Button '最小化窗口'}else{
  [FrameNative]::SetCursorPos([int]($minButton.Left+$minButton.Width/2),[int]($minButton.Top+$minButton.Height/2))|Out-Null;[FrameNative]::mouse_event(2,0,0,0,[UIntPtr]::Zero);[FrameNative]::mouse_event(4,0,0,0,[UIntPtr]::Zero);Pause 300
  }
  for($wait=0;$wait -lt 30 -and -not [FrameNative]::IsIconic($h);$wait++){Pause 100}
  Assert ([FrameNative]::IsIconic($h)) 'Custom minimize failed';if($cycle -eq 2){Pause 1700};Manage
  Assert (-not [FrameNative]::IsIconic($h)) 'Tray activation did not restore the minimized window'
 }
 [VaultTestNative]::SetForegroundWindow($h)|Out-Null;Pause 150
 $before=[FrameRect]::new();[FrameNative]::GetWindowRect($h,[ref]$before)|Out-Null;$screenBounds=(Main).Current.BoundingRectangle;$x=[int]($screenBounds.Left+110*$scale);$y=[int]($screenBounds.Top+30*$scale)
 Write-Output ('DRAG BEFORE: '+$before.Left+','+$before.Top+'; iconic='+[FrameNative]::IsIconic($h)+'; hit='+[FrameNative]::SendMessage($h,0x84,[IntPtr]::Zero,[IntPtr](($x -band 65535) -bor (($y -band 65535) -shl 16))))
 [FrameNative]::SetCursorPos($x,$y)|Out-Null;$point=[FramePoint]::new();$point.X=$x;$point.Y=$y;Write-Output ("DRAG target="+[FrameNative]::WindowFromPoint($point)+" expected=$h at=$x,$y");[FrameNative]::mouse_event(2,0,0,0,[UIntPtr]::Zero);Pause 150;[FrameNative]::SetCursorPos($x+80,$y+50)|Out-Null;Pause 150;[FrameNative]::mouse_event(4,0,0,0,[UIntPtr]::Zero);Pause 300
 $after=[FrameRect]::new();[FrameNative]::GetWindowRect($h,[ref]$after)|Out-Null;Write-Output ('DRAG AFTER: '+$after.Left+','+$after.Top);Assert ([Math]::Abs($after.Left-$before.Left) -gt 20 -or [Math]::Abs($after.Top-$before.Top) -gt 20) 'Custom header drag failed'
 Write-Output 'PASS: content-sized frameless create page, real rounded region, custom minimize/restore and mouse dragging.'
 SetEdit 0 'GUI-Master-Only!2026';SetEdit 1 'GUI-Master-Only!2026';Button '创建密码库';Pause 1600;Capture 'frame-main.png'
 $desktop=(Main).Current.BoundingRectangle
 Assert ((TextExists '分类 · 标签') -eq (($desktop.Width/$scale) -ge 720)) 'First management view used the previous page width'
 [FrameNative]::SetWindowPos($h,[IntPtr]::Zero,0,0,[int](660*$scale),[int]$desktop.Height,6)|Out-Null;Pause 250
 Assert (-not (TextExists '分类 · 标签')) 'Account list did not switch to the narrow layout'
 [FrameNative]::SetWindowPos($h,[IntPtr]::Zero,0,0,[int]$desktop.Width,[int]$desktop.Height,6)|Out-Null;Pause 250
 Assert ((TextExists '分类 · 标签') -eq (($desktop.Width/$scale) -ge 720)) 'Account list did not return to the wide layout'
 $area=[Windows.Forms.Screen]::FromHandle($h).WorkingArea
 [FrameNative]::SetWindowPos($h,[IntPtr]::Zero,$area.Left+36,$area.Top+36,0,0,5)|Out-Null
 Pause 250
 $before=(Main).Current.BoundingRectangle;[VaultTestNative]::SetForegroundWindow($h)|Out-Null;Pause 150;$corner=$null
 foreach($inset in @(4,5,5.5)){
  $point=[FramePoint]::new()
  $point.X=[int]($before.Right-$inset*$scale);$point.Y=[int]($before.Bottom-$inset*$scale)
  $packed=[IntPtr](($point.X -band 65535) -bor (($point.Y -band 65535) -shl 16))
  if([FrameNative]::WindowFromPoint($point) -eq $h -and [FrameNative]::SendMessage($h,0x84,[IntPtr]::Zero,$packed).ToInt32() -eq 17){$corner=$point;break}
 }
 Assert ($null -ne $corner) 'Rounded corner has no visible native resize target'
 [FrameNative]::SetCursorPos($corner.X,$corner.Y)|Out-Null;[FrameNative]::mouse_event(2,0,0,0,[UIntPtr]::Zero);Pause 150;[FrameNative]::SetCursorPos([int]($before.Right+30*$scale),[int]($before.Bottom+20*$scale))|Out-Null;Pause 200;[FrameNative]::mouse_event(4,0,0,0,[UIntPtr]::Zero);Pause 300;$resized=(Main).Current.BoundingRectangle;Write-Output ("RESIZE before=$($before.Width)x$($before.Height) after=$($resized.Width)x$($resized.Height)");Capture 'frame-resized.png';Assert ($resized.Width -gt $before.Width+20) 'Native corner resize failed'

 # Every edge/corner must return the native resize hit code, including negative-capable coordinates.
 $r=(Main).Current.BoundingRectangle
 foreach($case in @(@(1,($r.Height/2),10),@(($r.Width-2),($r.Height/2),11),@(($r.Width/2),1,12),@(($r.Width/2),($r.Height-2),15),@(1,1,13),@(($r.Width-2),1,14),@(1,($r.Height-2),16),@(($r.Width-2),($r.Height-2),17))){
  $x=[int]($r.X+$case[0]);$y=[int]($r.Y+$case[1]);$packed=[IntPtr](($x -band 65535) -bor (($y -band 65535) -shl 16))
  Assert ([FrameNative]::SendMessage($h,0x84,[IntPtr]::Zero,$packed).ToInt32() -eq $case[2]) "Resize border hit mismatch: $($case[2])"
 }
 $x=[int]($r.Right-62*$scale);$y=[int]($r.Top+30*$scale)
 Assert ([FrameNative]::SendMessage($h,0x84,[IntPtr]::Zero,[IntPtr](($x -band 65535) -bor (($y -band 65535) -shl 16))).ToInt32() -eq 9) 'Maximize hover does not expose native snap hit'
 # Drag the middle of the empty results area, away from the header.
 $x=[int]($r.Left+300*$scale);$y=[int]($r.Top+350*$scale)
 [FrameNative]::SetCursorPos($x,$y)|Out-Null;[FrameNative]::mouse_event(2,0,0,0,[UIntPtr]::Zero);Pause 100
 [FrameNative]::SetCursorPos($x+12,$y+12)|Out-Null;Pause 180;[FrameNative]::SetCursorPos($x+60,$y+40)|Out-Null;Pause 180;[FrameNative]::mouse_event(4,0,0,0,[UIntPtr]::Zero);Pause 300
 $dragged=(Main).Current.BoundingRectangle
 Assert ([Math]::Abs($dragged.X-$r.X) -gt 20 -or [Math]::Abs($dragged.Y-$r.Y) -gt 20) 'Blank results region cannot drag window'
 Capture 'frame-all-edges.png'
 Write-Output 'PASS: all four edges/corners expose native resizing, maximize exposes snap hit, and blank results region drags the window.'
 Button '最大化或还原窗口';Pause 300;Assert ([FrameNative]::IsZoomed($h)) 'Custom maximize failed';Button '最大化或还原窗口';Pause 400;Assert (-not [FrameNative]::IsZoomed($h) -and (Rounded $h)) 'Restore did not restore rounded shape'
 Button '新增账号';SetEdit 0 'Unsaved frame test';Button '关闭窗口';Assert (TextExists '未保存的修改') 'Custom close discarded draft';Button '继续编辑';Assert ((Edits)[0].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value -eq 'Unsaved frame test') 'Close guard lost draft';Button '取消编辑';Button '放弃修改'
 Write-Output 'PASS: custom maximize/restore restores corners; custom close preserves dirty-editor guard.'
 Button '最大化或还原窗口';Pause 300;Assert ([FrameNative]::IsZoomed($h)) 'Pre-lock maximize failed'
 Button '锁定';Pause 600;Assert (-not [FrameNative]::IsZoomed($h)) 'Lock kept the auth page maximized';Capture 'frame-unlock.png';$r=(Main).Current.BoundingRectangle;Assert (($r.Width/$scale) -le 480 -and ($r.Height/$scale) -le 290) 'Locked page did not shrink'
 Button '收起';OpenPopup;Assert (Rounded ([IntPtr](Popup).Current.NativeWindowHandle)) 'Popup rounding differs from main';Capture 'frame-popup-locked.png' (Popup);ToMain
 Assert (TextExists '解锁密码库') 'Navigation bypassed lock';Assert ($receiverBox.Text -eq '') 'Navigation filled receiver'
 Write-Output 'PASS: locked page shrinks; popup uses the same rounding; Ctrl+Shift+M opens locked management without filling or unlocking.'
 Unlock
 if([FrameNative]::IsZoomed($h)){Button '最大化或还原窗口'}
 $restored=(Main).Current.BoundingRectangle
 Assert ([Math]::Abs($restored.Width-$resized.Width) -le 3 -and [Math]::Abs($restored.Height-$resized.Height) -le 3) 'Lock/unlock lost the user-sized management dimensions'
 Button '新增账号';SetEdit 0 'Frame row';SetEdit 1 'frame.user';SetEdit 2 'Frame-Only!2026';Button '保存账号'
 Assert (TextExists '账号已保存。') 'Save feedback missing'
 (Edits)[0].SetFocus();[Windows.Forms.SendKeys]::SendWait('+{F10}');Pause 100;Button '删除账号'
 [Windows.Forms.SendKeys]::SendWait('{F2}');Pause 100
 Assert ((TextExists '确认删除') -and -not (TextExists '账号信息')) 'Edit shortcut bypassed delete confirmation'
 Button '取消删除'

 $row=@(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Button)|Where-Object{$_.Current.Name -like '*Frame row，*'})[0].Current.BoundingRectangle
 $bounds=(Main).Current.BoundingRectangle
 Assert ($row.Width -gt 0 -and $row.Right -le $bounds.Right) 'Account row retained the maximized viewport after restore'
 $x=[int]($row.Left+$row.Width/2);$y=[int]($row.Top+$row.Height/2)
 [VaultTestNative]::SetForegroundWindow($h)|Out-Null;Pause 150
 $target=[FramePoint]::new();$target.X=$x;$target.Y=$y
 Assert ([VaultTestNative]::GetForegroundWindow() -eq $h -and [FrameNative]::WindowFromPoint($target) -eq $h) 'Row double-click did not target the foreground test window'
 [FrameNative]::SetCursorPos($x,$y)|Out-Null
 foreach($i in 1..2){[FrameNative]::mouse_event(2,0,0,0,[UIntPtr]::Zero);Pause 30;[FrameNative]::mouse_event(4,0,0,0,[UIntPtr]::Zero);Pause 85};Pause 450
 Assert (TextExists '账号信息') 'Double-click did not edit account';Assert ((Edits)[0].GetCurrentPattern([Windows.Automation.ValuePattern]::Pattern).Current.Value -eq 'Frame row') 'Double-click selected a different account';Button '取消编辑'

 $row=@(Visible (Main) ([Windows.Automation.AutomationElement]::ControlTypeProperty) ([Windows.Automation.ControlType]::Button)|Where-Object{$_.Current.Name -like '*Frame row，*'})[0].Current.BoundingRectangle
 $r=(Main).Current.BoundingRectangle;$x=[int]($row.Left+$row.Width/2);$y=[int]($row.Top+$row.Height/2)
 [FrameNative]::SetCursorPos($x,$y)|Out-Null;[FrameNative]::mouse_event(2,0,0,0,[UIntPtr]::Zero);Pause 80;[FrameNative]::SetCursorPos($x-12,$y-12)|Out-Null;Pause 150;[FrameNative]::SetCursorPos($x-60,$y-40)|Out-Null;Pause 150;[FrameNative]::mouse_event(4,0,0,0,[UIntPtr]::Zero);Pause 200
 $moved=(Main).Current.BoundingRectangle;Assert ([Math]::Abs($moved.X-$r.X) -gt 20 -or [Math]::Abs($moved.Y-$r.Y) -gt 20) 'Account row cannot drag window'
 # Actual NC mouse clicks must still activate the custom maximize button.
 $r=(Main).Current.BoundingRectangle;$x=[int]($r.Right-62*$scale);$y=[int]($r.Top+30*$scale)
 [FrameNative]::SetCursorPos($x,$y)|Out-Null;[FrameNative]::mouse_event(2,0,0,0,[UIntPtr]::Zero);[FrameNative]::mouse_event(4,0,0,0,[UIntPtr]::Zero);Pause 400
 Assert ([FrameNative]::IsZoomed($h)) 'Native maximize hover blocked mouse click';Button '最大化或还原窗口'
 $r=(Main).Current.BoundingRectangle;$x=[int]($r.Left+100*$scale);$y=[int]($r.Top+30*$scale)
 [FrameNative]::SetCursorPos($x,$y)|Out-Null
 foreach($i in 1..2){[FrameNative]::mouse_event(2,0,0,0,[UIntPtr]::Zero);[FrameNative]::mouse_event(4,0,0,0,[UIntPtr]::Zero);Pause 80};Pause 300
 Assert ([FrameNative]::IsZoomed($h)) 'Double-click header did not maximize';Button '最大化或还原窗口'
 Write-Output 'PASS: user size survives lock/unlock; real double-click edits correct account; mouse maximize and header double-click work.'

}finally{StopApp;$form.Dispose()}
