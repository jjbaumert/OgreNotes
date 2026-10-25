# Copyright (c) 2026 Joel Baumert. All Rights Reserved.
"""Configure a real ImageMagick librsvg delegate without exposing rsvg-convert to the CLI."""
from pathlib import Path
import os
import shutil
import shlex
import subprocess
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
# The delegate gets input, output and density from ImageMagick. Use an
# absolute interpreter and renderer so the CLI can see only ImageMagick.
wrapper.write_text('#!' + sys.executable + '\n' +
    'import subprocess,sys\n' +
    f'subprocess.run([{rsvg!r}, sys.argv[1], "--output", sys.argv[2], '
    '"--zoom", str(float(sys.argv[3])/96)], check=True)\n')
wrapper.chmod(0o700)
command = shlex.quote(str(wrapper)) + " '%s' '%s' '%s'"
(root / 'config' / 'delegates.xml').write_text(
    '<delegatemap><delegate decode="svg:decode" stealth="True" command=' +
    quoteattr(command) + '/></delegatemap>\n')
print('ImageMagick-only CLI PATH:', root / 'bin')

# Run the same raw-RGB capability probe as mermaid_cli, but keep stderr visible
# so a broken system delegate reports why instead of only returning "unsupported".
probe = root / 'probe.svg'
probe.write_text('''<svg xmlns="http://www.w3.org/2000/svg" width="24" height="8"><rect width="24" height="8" fill="#fff"/><svg width="8" height="8" overflow="hidden"><rect width="16" height="8" fill="#0f0"/></svg><defs><mask id="probe" maskUnits="userSpaceOnUse" x="8" y="0" width="8" height="8"><rect x="8" width="8" height="8" fill="#fff"/><rect x="8" width="4" height="8" fill="#000"/></mask></defs><rect x="8" width="8" height="8" fill="#f00" mask="url(#probe)"/><svg x="16" width="8" height="8" overflow="hidden"><rect width="8" height="8" fill="#00f" stroke="#f00" stroke-width="10" paint-order="stroke"/></svg></svg>''')
environment = os.environ.copy()
environment['MAGICK_CONFIGURE_PATH'] = str(root / 'config')
environment['PATH'] = str(root / 'bin') + os.pathsep + environment['PATH']
result = subprocess.run(
    [magick, '-density', '96', str(probe), '-depth', '8', 'rgb:-'],
    env=environment, capture_output=True)
if result.returncode:
    sys.stderr.buffer.write(result.stderr)
    raise SystemExit(f'ImageMagick delegate probe exited with {result.returncode}')
if len(result.stdout) != 24 * 8 * 3:
    raise SystemExit(f'ImageMagick delegate probe returned {len(result.stdout)} bytes, expected 576')
for x, expected in ((4, bytes((0, 255, 0))), (9, bytes((255, 255, 255))),
                    (14, bytes((255, 0, 0))), (20, bytes((0, 0, 255)))):
    actual = result.stdout[x * 3:x * 3 + 3]
    if actual != expected:
        raise SystemExit(f'ImageMagick delegate probe pixel {x}: {list(actual)}, expected {list(expected)}')
print('ImageMagick SVG delegate compatibility probe passed')
