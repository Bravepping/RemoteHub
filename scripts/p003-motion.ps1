param([ValidateRange(1,40)][int]$Seconds=16)
# Development-only visible motion source, captured through the real Windows desktop/DXGI path.
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
Add-Type -MemberDefinition '[System.Runtime.InteropServices.DllImport("user32.dll")] public static extern bool SetProcessDPIAware();' -Name ProbeDpi -Namespace RemoteHubValidation
[void][RemoteHubValidation.ProbeDpi]::SetProcessDPIAware()
$form=New-Object System.Windows.Forms.Form
$form.AutoScaleMode='None'
$form.Text='RemoteHub P0-03 - VISIBLE DESKTOP MOTION FIXTURE - closes automatically'
$form.StartPosition='Manual'
$form.Location=New-Object System.Drawing.Point(80,80)
$form.ClientSize=New-Object System.Drawing.Size(1280,720)
$form.TopMost=$true
$form.Add_Shown({
  $rectangle=$form.RectangleToScreen($form.ClientRectangle)
  [Console]::WriteLine((@{left=$rectangle.X;top=$rectangle.Y;width=$rectangle.Width;height=$rectangle.Height;fixture='visible color bars and moving block'}|ConvertTo-Json -Compress))
})
$clock=[Diagnostics.Stopwatch]::StartNew()
$colors=@([Drawing.Color]::White,[Drawing.Color]::Yellow,[Drawing.Color]::Cyan,[Drawing.Color]::Lime,[Drawing.Color]::Magenta,[Drawing.Color]::Red,[Drawing.Color]::Blue,[Drawing.Color]::Black)
$form.Add_Paint({
  param($sender,$event)
  for($i=0;$i -lt 8;$i++) {
    $brush=New-Object Drawing.SolidBrush($colors[$i])
    $event.Graphics.FillRectangle($brush,($i*160),0,160,560)
    $brush.Dispose()
  }
  $event.Graphics.FillRectangle([Drawing.Brushes]::Navy,0,560,1280,160)
  $x=[int](($clock.ElapsedMilliseconds/3)%1150)
  $event.Graphics.FillRectangle([Drawing.Brushes]::White,$x,600,120,65)
  $label='Actual Windows desktop motion | elapsed '+$clock.ElapsedMilliseconds+' ms | auto-close '+$Seconds+' s'
  $event.Graphics.DrawString($label,$form.Font,[Drawing.Brushes]::White,16,575)
})
$timer=New-Object Windows.Forms.Timer
$timer.Interval=33
$timer.Add_Tick({if($clock.Elapsed.TotalSeconds -ge $Seconds){$form.Close()}else{$form.Invalidate()}})
$timer.Start()
try {[Windows.Forms.Application]::Run($form)} finally {$timer.Dispose();$form.Dispose()}
