#!/usr/bin/env python3
"""Fetch the datasheets of the S510's parts into target/datasheets/ and check each against the copy the docs cite.

  tools/datasheets.py            download what is missing, then check every file
  tools/datasheets.py --check    no network: check what is in target/datasheets/
  tools/datasheets.py selftest   a fake download, no network (check.sh)

The vendors' licences forbid redistributing their PDFs, so none is in the repository: docs/datasheets.md lists the
parts, and this fetches the documents from their publishers. Some sites refuse scripts (a browser check): for those it
prints the link to open in a browser and the name to save the file under, then checks it on the next run. A hash that
differs from the pinned one is reported, not refused: the vendor may have published a new revision, and the docs cite
tables of the revision pinned here.
"""
import hashlib, os, sys, urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEST = os.path.join(ROOT, 'target', 'datasheets')
UA = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15'

# (file, document, url, sha256 of the copy the docs cite or None, by hand: the site refuses scripts)
SHEETS = [
    ('STM32H750IB-DS12556.pdf', 'ST DS12556 Rev 8, STM32H750xB datasheet',
     'https://www.st.com/resource/en/datasheet/stm32h750ib.pdf',
     '40d31484a7c50adc849c455f7b80f29d6a99dc11e3913f2fd92ef346ece7bb53', True),
    ('AS4C32M16MS.pdf', 'Alliance Memory AS4C32M16MS rev 1.0, 512 Mbit mobile SDR SDRAM',
     'https://www.alliancememory.com/wp-content/uploads/'
     'AS4C32M16MS-6BIN_512M-LOW-POWER-SDRAM_AS4C32M16MS-7BCN__AS4C32M16MS-6BIN-rev1.0-June-2016.pdf',
     'f65bafb2985d7e6f6bb3f8ecd646cfd7bed8eeedcaf8455eb572e0048857f5ab', False),
    ('USB3320.pdf', 'SMSC USB3320 datasheet rev 1.0, ULPI high-speed USB PHY',
     'https://datasheet4u.com/pdf/737354/USB3320.pdf',
     '38362d1b0cf6d04d046e61146afc1376a370949ca7321707c979a44e38e4ac47', False),
    ('nRF52832-PS-v1.9.pdf', 'Nordic nRF52832 Product Specification v1.9',
     'https://mm.digikey.com/Volume0/opasdata/d220001/medias/docus/8881/4823_nRF52832_PS_v1.9.pdf',
     '2ea4a573c232d2b2b52056e718d4bc173e2cc019b08394ce03c848b15d577093', False),
    ('u-blox-M10-SPG-5.10-UBX-21035062.pdf', 'u-blox M10 SPG 5.10 interface description, UBX-21035062',
     'https://content.u-blox.com/sites/default/files/u-blox-M10-SPG-5.10_InterfaceDescription_UBX-21035062.pdf',
     'b20859d86b043458494a3473c28f03ec8ad1e9e9d6a61ea2d220aa0f739248b6', False),
    ('ST7789V-V1.3.pdf', 'Sitronix ST7789V datasheet V1.3, the display controller',
     'https://newhavendisplay.com/content/datasheets/ST7789V.pdf',
     '8ecf0e438aa25554efc473a4cfe6e436f3fb33e68cd876b25041557235d04ca1', False),
    ('ST1624-v1.1.pdf', 'Sitronix ST1624 datasheet v1.1, the touch controller',
     'https://www.crystalfontz.com/controllers/uploaded/Sitronix_ST1624_Datasheet_v1.1.pdf',
     '920e856388aa8777f5492fb16987bcfc77b2a5b29018f9ea327e5e0fd595ac31', False),
    ('ETA6003-v2.5.1.pdf', 'ETA Solutions ETA6003 v2.5.1, the battery charger',
     'https://e2e.ti.com/cfs-file/__key/communityserver-discussions-components-files/196/ETA6003-v2.5.1.pdf',
     '1fb0e1b3bc6deee383df939c6e80d1599f1fc59562da9fb7c30d21370d4f8389', False),
]


def get(url):
    with urllib.request.urlopen(urllib.request.Request(url, headers={'User-Agent': UA}), timeout=60) as r:
        return r.read()


def save(name, data):
    """Write DEST/name whole or not at all: a temporary file, then a rename."""
    os.makedirs(DEST, exist_ok=True)
    tmp = os.path.join(DEST, name + '.part')
    with open(tmp, 'wb') as f:
        f.write(data)
    os.replace(tmp, os.path.join(DEST, name))


def main(args, fetch=get):
    """Returns the number of files missing or not a PDF (the exit status)."""
    bad = 0
    for name, doc, url, sha, by_hand in SHEETS:
        path = os.path.join(DEST, name)
        if not os.path.exists(path) and not by_hand and '--check' not in args:
            try:
                data = fetch(url)
            except Exception as e:
                data = b''
                print(f'{name}: download failed ({e})')
            if data.startswith(b'%PDF'):
                save(name, data)
            elif data:
                print(f'{name}: the site answered with something other than a PDF; nothing saved')
        if not os.path.exists(path):
            how = 'open in a browser and save as' if by_hand else 'download failed; fetch by hand as'
            print(f'MISSING  {doc}\n         {how} {path}\n         {url}')
            bad += 1
            continue
        data = open(path, 'rb').read()
        got = hashlib.sha256(data).hexdigest()
        if not data.startswith(b'%PDF'):
            print(f'NOT PDF  {name}: delete it and fetch it again')
            bad += 1
        elif sha is None:
            print(f'UNPINNED {name}: sha256 {got} (pin it in tools/datasheets.py)')
        elif got != sha:
            print(f'CHANGED  {name}: not the copy the docs cite ({doc}); a new revision? check the citations')
        else:
            print(f'OK       {name}')
    return bad


def selftest():
    """Fake downloads: a PDF lands whole, an HTML page is refused, a by-hand file is asked for, a change is reported."""
    import contextlib, io, tempfile
    global DEST, SHEETS
    pdf, html = b'%PDF-1.4 fake', b'<html>a browser check</html>'
    served = {'u/a': pdf, 'u/b': html}
    old = DEST, SHEETS
    try:
        with tempfile.TemporaryDirectory() as d:
            DEST = d
            SHEETS = [('a.pdf', 'A', 'u/a', hashlib.sha256(pdf).hexdigest(), False),
                      ('b.pdf', 'B', 'u/b', None, False),
                      ('c.pdf', 'C', 'u/c', None, True)]
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                assert main([], served.__getitem__) == 2, out.getvalue()
            assert sorted(os.listdir(d)) == ['a.pdf'], os.listdir(d)
            assert 'OK       a.pdf' in out.getvalue() and 'open in a browser' in out.getvalue()
            open(os.path.join(d, 'a.pdf'), 'wb').write(pdf + b'!')
            open(os.path.join(d, 'c.pdf'), 'wb').write(pdf)
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                assert main(['--check'], served.__getitem__) == 1, out.getvalue()
            assert 'CHANGED  a.pdf' in out.getvalue() and 'UNPINNED c.pdf' in out.getvalue(), out.getvalue()
    finally:
        DEST, SHEETS = old
    index = open(os.path.join(ROOT, 'docs', 'datasheets.md')).read()
    for name, _, url, _, _ in SHEETS:
        assert url in index, f'{name}: its link is not in docs/datasheets.md'
    print('datasheets selftest OK')


if __name__ == '__main__':
    if sys.argv[1:] == ['selftest']:
        selftest()
    else:
        sys.exit(1 if main(sys.argv[1:]) else 0)
