param([string]$Destination=(Join-Path $PSScriptRoot '..\assets'))
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Drawing
$frames=@()
foreach($size in @(256,128,64,48,32,24,20,16)){
 $bitmap=[Drawing.Bitmap]::new($size,$size,[Drawing.Imaging.PixelFormat]::Format32bppArgb)
 $graphics=[Drawing.Graphics]::FromImage($bitmap)
 $path=[Drawing.Drawing2D.GraphicsPath]::new()
 $brush=[Drawing.SolidBrush]::new([Drawing.ColorTranslator]::FromHtml('#11675f'))
 $pen=[Drawing.Pen]::new([Drawing.Color]::White,3.5)
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
  $graphics.DrawEllipse($pen,18,18,14,14)
  $graphics.DrawLine($pen,30,30,43,43);$graphics.DrawLine($pen,36,36,40,32);$graphics.DrawLine($pen,41,41,45,37)
  $graphics.DrawBezier($pen,14,43,22,51,42,51,50,43)
  $bitmap.Save($stream,[Drawing.Imaging.ImageFormat]::Png)
  $frames+=,@{Size=$size;Bytes=$stream.ToArray()}
  if($size -eq 256){$bitmap.Save((Join-Path $Destination 'keyisle-preview.png'),[Drawing.Imaging.ImageFormat]::Png)}
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
Write-Output 'Generated Keyisle icon: 16, 20, 24, 32, 48, 64, 128 and 256 pixels.'

$banner=[Drawing.Bitmap]::new(164,314,[Drawing.Imaging.PixelFormat]::Format24bppRgb)
$canvas=[Drawing.Graphics]::FromImage($banner)
$logo=[Drawing.Image]::FromFile((Join-Path $Destination 'keyisle-preview.png'))
$titleFont=[Drawing.Font]::new('Segoe UI',18,[Drawing.FontStyle]::Bold)
$smallFont=[Drawing.Font]::new('Segoe UI',7)
$ink=[Drawing.SolidBrush]::new([Drawing.ColorTranslator]::FromHtml('#183431'))
$muted=[Drawing.SolidBrush]::new([Drawing.ColorTranslator]::FromHtml('#61747c'))
$format=[Drawing.StringFormat]::new();$format.Alignment=[Drawing.StringAlignment]::Center
try{
 $canvas.Clear([Drawing.ColorTranslator]::FromHtml('#f7f9fb'))
 $canvas.InterpolationMode=[Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
 $canvas.DrawImage($logo,40,78,84,84)
 $canvas.DrawString('Keyisle',$titleFont,$ink,[Drawing.RectangleF]::new(0,180,164,38),$format)
 $canvas.DrawString('LOCAL PASSWORD VAULT',$smallFont,$muted,[Drawing.RectangleF]::new(0,226,164,24),$format)
 $banner.Save((Join-Path $Destination 'keyisle-installer.bmp'),[Drawing.Imaging.ImageFormat]::Bmp)
}finally{$canvas.Dispose();$banner.Dispose();$logo.Dispose();$titleFont.Dispose();$smallFont.Dispose();$ink.Dispose();$muted.Dispose();$format.Dispose()}
