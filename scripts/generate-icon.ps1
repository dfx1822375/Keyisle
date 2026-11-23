param([string]$Destination=(Join-Path $PSScriptRoot '..\assets'))
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Drawing
$frames=@()
foreach($size in @(512,256,128,96,72,64,48,40,32,24,20,16)){
 $bitmap=[Drawing.Bitmap]::new($size,$size,[Drawing.Imaging.PixelFormat]::Format32bppArgb)
 $graphics=[Drawing.Graphics]::FromImage($bitmap)
 $path=[Drawing.Drawing2D.GraphicsPath]::new()
 $brush=[Drawing.SolidBrush]::new([Drawing.ColorTranslator]::FromHtml('#11675f'))
 $pen=[Drawing.Pen]::new([Drawing.Color]::White,$(if($size -le 24){4.5}else{3.5}))
 $stream=[IO.MemoryStream]::new()
 try{
  $graphics.Clear([Drawing.Color]::Transparent)
  $graphics.SmoothingMode=[Drawing.Drawing2D.SmoothingMode]::AntiAlias
  $graphics.PixelOffsetMode=[Drawing.Drawing2D.PixelOffsetMode]::HighQuality
  $graphics.ScaleTransform($size/64.0,$size/64.0)
  $path.AddArc(0,0,32,32,180,90);$path.AddArc(32,0,32,32,270,90)
  $path.AddArc(32,32,32,32,0,90);$path.AddArc(0,32,32,32,90,90);$path.CloseFigure()
  $graphics.FillPath($brush,$path)
  $pen.StartCap=[Drawing.Drawing2D.LineCap]::Round;$pen.EndCap=[Drawing.Drawing2D.LineCap]::Round
  $graphics.DrawEllipse($pen,16.5,15.5,15,15)
  $dot=[Drawing.SolidBrush]::new([Drawing.Color]::White)
  if($size -ge 32){$graphics.FillEllipse($dot,20.9,19.9,3.2,3.2)}
  $dot.Dispose()
  $graphics.DrawLine($pen,29.5,28.5,44,43);$graphics.DrawLine($pen,36.5,35.5,40.7,31.3);$graphics.DrawLine($pen,41.5,40.5,45.7,36.3)
  $graphics.DrawBezier($pen,13,46.5,20.5,53,43.5,53,51,46.5)
  $wavePen=[Drawing.Pen]::new([Drawing.Color]::FromArgb(184,255,255,255),2.4)
  $wavePen.StartCap=[Drawing.Drawing2D.LineCap]::Round;$wavePen.EndCap=[Drawing.Drawing2D.LineCap]::Round
  if($size -ge 32){$graphics.DrawBezier($wavePen,20,51.5,25.5,54.7,38.5,54.7,44,51.5)}
  $wavePen.Dispose()
  if($size -eq 512){
   $bitmap.Save((Join-Path $Destination 'keyisle-preview.png'),[Drawing.Imaging.ImageFormat]::Png)
  }else{
   $bitmap.Save($stream,[Drawing.Imaging.ImageFormat]::Png)
   $frames+=,@{Size=$size;Bytes=$stream.ToArray()}
  }
 }finally{$graphics.Dispose();$bitmap.Dispose();$pen.Dispose();$brush.Dispose();$path.Dispose();$stream.Dispose()}
}
$file=[IO.File]::Create((Join-Path $Destination 'keyisle.ico'))
$writer=[IO.BinaryWriter]::new($file)
try{
 $writer.Write([uint16]0);$writer.Write([uint16]1);$writer.Write([uint16]$frames.Count)
 $offset=6+16*$frames.Count
 foreach($frame in $frames){
  $dimension=if($frame.Size -eq 256){0}else{$frame.Size}
  $writer.Write([byte]$dimension);$writer.Write([byte]$dimension);$writer.Write([byte]0);$writer.Write([byte]0)
  $writer.Write([uint16]1);$writer.Write([uint16]32);$writer.Write([uint32]$frame.Bytes.Length);$writer.Write([uint32]$offset)
  $offset+=$frame.Bytes.Length
 }
 foreach($frame in $frames){$writer.Write([byte[]]$frame.Bytes)}
}finally{$writer.Dispose();$file.Dispose()}
Write-Output 'Generated Keyisle icon with 11 DPI-specific frames and a 512 px preview.'

# NSIS scales its sidebar to the dialog. Start with a 3x image for high-DPI Windows.
$scale=3
$banner=[Drawing.Bitmap]::new((164*$scale),(314*$scale),[Drawing.Imaging.PixelFormat]::Format24bppRgb)
$canvas=[Drawing.Graphics]::FromImage($banner)
$logo=[Drawing.Image]::FromFile((Join-Path $Destination 'keyisle-preview.png'))
$titleFont=[Drawing.Font]::new('Segoe UI',18,[Drawing.FontStyle]::Bold)
$ink=[Drawing.SolidBrush]::new([Drawing.ColorTranslator]::FromHtml('#183431'))
$format=[Drawing.StringFormat]::new();$format.Alignment=[Drawing.StringAlignment]::Center
try{
 $canvas.Clear([Drawing.ColorTranslator]::FromHtml('#f7f9fb'))
 $canvas.ScaleTransform($scale,$scale)
 $canvas.InterpolationMode=[Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
 $canvas.PixelOffsetMode=[Drawing.Drawing2D.PixelOffsetMode]::HighQuality
 $canvas.TextRenderingHint=[Drawing.Text.TextRenderingHint]::AntiAliasGridFit
 $canvas.DrawImage($logo,34,78,96,96)
 $canvas.DrawString('Keyisle',$titleFont,$ink,[Drawing.RectangleF]::new(0,190,164,38),$format)
 $banner.Save((Join-Path $Destination 'keyisle-installer.bmp'),[Drawing.Imaging.ImageFormat]::Bmp)
}finally{$canvas.Dispose();$banner.Dispose();$logo.Dispose();$titleFont.Dispose();$ink.Dispose();$format.Dispose()}
