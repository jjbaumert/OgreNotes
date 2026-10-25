# Copyright (c) 2026 Joel Baumert. All Rights Reserved.
"""Configure a real ImageMagick librsvg delegate without exposing rsvg-convert to the CLI."""
from pathlib import Path
import shutil
import shlex
import sys
from xml.sax.saxutils import quoteattr

root = Path(sys.argv[1]).resolve()
(root / 'config').mkdir(parents=True, exist_ok=True)
(root / 'bin').mkdir(exist_ok=True)
rsvg = shutil.which('rsvg-convert')
magick = shutil.which('magick') or shutil.which('convert')
if not rsvg or not magick:
    raise SystemExit('This regression requires rsvg-convert and ImageMagick.')
link = root / 'bin' / Path(magick).name
if link.is_symlink():
    link.unlink()
link.symlink_to(magick)
wrapper = root / 'config' / 'rasterize'
# The delegate gets input, output and density from ImageMagick. Use absolute
# tools so the CLI can see only ImageMagick. Its capability probe asks for raw
# RGB, so translate the intermediate librsvg PNG through ImageMagick.
wrapper.write_text(
    '#!' + sys.executable + '\n' +
    'import subprocess,sys,tempfile\n' +
    f'rsvg={rsvg!r}\nmagick={magick!r}\n' +
    'args=[rsvg, sys.argv[1], "--zoom", str(float(sys.argv[3])/96)]\n' +
    'if sys.argv[2] == "rgb:-":\n' +
    '  with tempfile.NamedTemporaryFile(suffix=".png") as image:\n' +
    '    subprocess.run(args + ["--output", image.name], check=True)\n' +
    '    subprocess.run([magick, image.name, "-depth", "8", "rgb:-"], stdout=sys.stdout.buffer, check=True)\n' +
    'else:\n' +
    '  subprocess.run(args + ["--output", sys.argv[2]], check=True)\n')
wrapper.chmod(0o700)
command = shlex.quote(str(wrapper)) + " '%s' '%s' '%s'"
(root / 'config' / 'delegates.xml').write_text(
    '<delegatemap><delegate decode="svg:decode" stealth="True" command=' +
    quoteattr(command) + '/></delegatemap>\n')
print('ImageMagick-only CLI PATH:', root / 'bin')
