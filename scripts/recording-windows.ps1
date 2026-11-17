param([Parameter(Mandatory=$true)][string]$RecorderExe)
$ErrorActionPreference='Stop'
[Console]::OutputEncoding=[Text.UTF8Encoding]::new($false)
Add-Type -AssemblyName System.Windows.Forms
Add-Type -ReferencedAssemblies System.Windows.Forms @'
using System;using System.Runtime.InteropServices;using System.Windows.Forms;
public class ShortcutReceiver:Form {
 public int Hotkeys;
 protected override void WndProc(ref Message msg){if(msg.Msg==0x312)Hotkeys++;base.WndProc(ref msg);}
 [DllImport("user32.dll")] public static extern void keybd_event(byte key,byte scan,uint flags,UIntPtr extra);
 [DllImport("user32.dll")] public static extern bool RegisterHotKey(IntPtr h,int id,uint mods,uint key);
 [DllImport("user32.dll")] public static extern bool UnregisterHotKey(IntPtr h,int id);
 [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
}
'@
function Pump([int]$ms){$watch=[Diagnostics.Stopwatch]::StartNew();while($watch.ElapsedMilliseconds -lt $ms){[Windows.Forms.Application]::DoEvents();Start-Sleep -Milliseconds 10}}
$form=[ShortcutReceiver]::new();$form.Text='Isolated shortcut recording';$form.Width=400;$form.Height=180
$input=[Windows.Forms.TextBox]::new();$form.Controls.Add($input);$form.Show();$form.Activate();$input.Focus()|Out-Null
$process=$null
try {
 if(-not [ShortcutReceiver]::RegisterHotKey($form.Handle,98,3,0x51)){throw 'Cannot reserve test shortcut'}
 [ShortcutReceiver]::SetForegroundWindow($form.Handle)|Out-Null;Pump 200
 $start=[Diagnostics.ProcessStartInfo]::new();$start.FileName=$RecorderExe;$start.Arguments=$form.Handle.ToInt64().ToString();$start.UseShellExecute=$false;$start.CreateNoWindow=$true;$start.RedirectStandardOutput=$true;$start.RedirectStandardError=$true
 $process=[Diagnostics.Process]::Start($start)
 $ready=$process.StandardOutput.ReadLineAsync()
 for($i=0;$i -lt 500 -and -not $ready.IsCompleted;$i++){Pump 10}
 if(-not $ready.IsCompleted -or $ready.Result -ne 'READY'){throw 'Recorder did not become ready'}
 foreach($key in @(0x11,0x12,0x51)){[ShortcutReceiver]::keybd_event($key,0,0,[UIntPtr]::Zero);Pump 60}
 foreach($key in @(0x51,0x12,0x11)){[ShortcutReceiver]::keybd_event($key,0,2,[UIntPtr]::Zero);Pump 60}
 $line=$process.StandardOutput.ReadLineAsync()
 for($i=0;$i -lt 500 -and -not $line.IsCompleted;$i++){Pump 10}
 if(-not $line.IsCompleted -or $line.Result -ne 'CAPTURED Ctrl+Alt+Q'){throw 'Occupied shortcut was not captured'}
 if(-not $process.WaitForExit(3000) -or $process.ExitCode -ne 0){throw 'Recorder did not release its hook'}
 if($form.Hotkeys -ne 0){throw 'Recording triggered the other application hotkey'}
 Write-Output 'PASS: temporary recorder captures an occupied shortcut, suppresses its external action and exits cleanly.'
 [ShortcutReceiver]::keybd_event(0x11,0,0,[UIntPtr]::Zero);[ShortcutReceiver]::keybd_event(0x12,0,0,[UIntPtr]::Zero);[ShortcutReceiver]::keybd_event(0x51,0,0,[UIntPtr]::Zero);Pump 100
 [ShortcutReceiver]::keybd_event(0x51,0,2,[UIntPtr]::Zero);[ShortcutReceiver]::keybd_event(0x12,0,2,[UIntPtr]::Zero);[ShortcutReceiver]::keybd_event(0x11,0,2,[UIntPtr]::Zero);Pump 100
 if($form.Hotkeys -lt 1){throw 'Temporary recorder left keyboard interception active'}
 Write-Output 'PASS: external shortcut resumes after recording.'
}finally{
 foreach($key in @(0x51,0x12,0x11)){[ShortcutReceiver]::keybd_event($key,0,2,[UIntPtr]::Zero)}
 [ShortcutReceiver]::UnregisterHotKey($form.Handle,98)|Out-Null
 if($process -and -not $process.HasExited){$process.Kill();$process.WaitForExit()}
 $form.Dispose()
}
