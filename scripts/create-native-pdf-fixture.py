"""Create local-only, deterministic PDF fixtures for native preview acceptance.

Requires ReportLab and pypdf; no network, private document, external link or
executable PDF action. Bundled fixture bytes also work without Python at build.
"""
import argparse
import hashlib
import io
import json
from pathlib import Path

from pypdf import PdfReader, PdfWriter
from pypdf.generic import ArrayObject, ByteStringObject
from reportlab.lib.colors import HexColor, white
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from reportlab.pdfgen import canvas

ROOT = Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser()
parser.add_argument("--font", type=Path)
args = parser.parse_args()
font = "Helvetica"
if args.font:
    font = "FixtureSans"
    pdfmetrics.registerFont(TTFont(font, str(args.font)))

buffer = io.BytesIO()
c = canvas.Canvas(buffer, pagesize=(480, 640), invariant=1, pageCompression=1)
c.setTitle("PasteRS synthetic native preview acceptance")
c.setAuthor("PasteRS test fixture")
c.setSubject("Three pages; mixed sizes; internal navigation only")
ink, muted, accent = map(HexColor, ("#17283D", "#52657B", "#135CC5"))


def text(x, y, value, size=12, color=ink):
    c.setFillColor(color)
    c.setFont(font, size)
    c.drawString(x, y, value)


def page_header(width, height, number, title, subtitle):
    c.setPageSize((width, height))
    c.setFillColor(HexColor("#F3F6FB"))
    c.rect(0, 0, width, height, fill=1, stroke=0)
    c.setFillColor(accent)
    c.rect(0, height - 12, width, 12, fill=1, stroke=0)
    text(36, height - 47, "PASTERS  /  NATIVE PREVIEW QA", 10, muted)
    text(36, height - 83, title, 25)
    text(36, height - 108, subtitle, 11, muted)
    c.setStrokeColor(HexColor("#CAD5E2"))
    c.line(36, 45, width - 36, 45)
    text(36, 27, "Synthetic only - no clipboard content", 9, muted)
    text(width - 96, 27, f"PAGE {number} / 3", 9, muted)


page_header(480, 640, 1, "Portrait overview", "480 x 640 pt | selectable text | internal link")
c.bookmarkPage("overview")
c.addOutlineEntry("1. Portrait overview", "overview")
text(36, 470, "Acceptance token: PORTRAIT-ONE", 14)
for n, line in enumerate([
    "The card must show this first page as a thumbnail.",
    "Full preview must retain all three original pages.",
    "Scroll, zoom and search must not reset on refresh.",
]):
    text(36, 437 - n * 23, line, 11)
c.setFillColor(accent)
c.roundRect(36, 308, 252, 40, 8, fill=1, stroke=0)
text(49, 322, "Jump to detail page 3", 12, white)
c.linkRect("Jump to detail page 3", "detail", (36, 308, 288, 348), relative=0, thickness=0)
for n, (color, label) in enumerate([
    ("#D54743", "RED - top"), ("#135CC5", "BLUE - middle"), ("#27825A", "GREEN - bottom")
]):
    c.setFillColor(HexColor(color))
    c.roundRect(36, 230 - 53 * n, 408, 38, 6, fill=1, stroke=0)
    text(49, 243 - 53 * n, label, 11, white)
c.showPage()

page_header(720, 480, 2, "Landscape table", "720 x 480 pt | column alignment | page boundaries")
c.bookmarkPage("table")
c.addOutlineEntry("2. Landscape table", "table")
text(36, 329, "Acceptance token: LANDSCAPE-TWO", 14)
rows = [
    ("Check", "Expected result", "Marker"),
    ("Page width", "Landscape without cropping", "WIDE-02"),
    ("Text selection", "Separate readable columns", "TEXT-02"),
    ("Refresh", "Page and zoom stay unchanged", "STABLE-02"),
    ("Original data", "Export retains all pages", "BYTES-02"),
]
for i, row in enumerate(rows):
    y = 271 - i * 39
    c.setFillColor(accent if i == 0 else HexColor("#FFFFFF" if i % 2 else "#E6EDF6"))
    c.rect(36, y, 648, 39, fill=1, stroke=0)
    for x, value in zip((48, 194, 568), row):
        text(x, y + 14, value, 11, white if i == 0 else ink)
c.showPage()

page_header(480, 640, 3, "Vector detail", "480 x 640 pt | zoom target | return link")
c.bookmarkPage("detail")
c.addOutlineEntry("3. Vector detail", "detail")
text(36, 470, "Acceptance token: DETAIL-THREE", 14)
c.setStrokeColor(HexColor("#A9BDDA"))
c.setLineWidth(0.5)
for n in range(17):
    c.line(48 + n * 24, 172, 48 + n * 24, 412)
for n in range(11):
    c.line(48, 172 + n * 24, 432, 172 + n * 24)
c.setStrokeColor(accent)
c.setLineWidth(2)
c.circle(240, 292, 86, stroke=1, fill=0)
text(139, 296, "VECTOR ZOOM TARGET", 13)
text(145, 271, "Select this text on page 3", 10)
text(36, 106, "Return to portrait overview", 12, accent)
c.linkRect("Return to page 1", "overview", (36, 99, 260, 123), relative=0, thickness=0)
c.showPage()
c.save()
pdf = buffer.getvalue()
reader = PdfReader(io.BytesIO(pdf))
assert len(reader.pages) == 3
assert [tuple(float(x) for x in page.mediabox[2:]) for page in reader.pages] == [
    (480.0, 640.0), (720.0, 480.0), (480.0, 640.0)
]
for page, token in zip(reader.pages, ("PORTRAIT-ONE", "LANDSCAPE-TWO", "DETAIL-THREE")):
    assert token in page.extract_text()
    for annotation in page.get("/Annots", []):
        # Links must be internal destinations, never URI / JavaScript / Launch.
        link = annotation.get_object()
        assert "/A" not in link and "/Dest" in link

output = ROOT / "output/pdf/native-preview-acceptance.pdf"
fixtures = ROOT / "apps/desktop/src-tauri/fixtures"
output.parent.mkdir(parents=True, exist_ok=True)
fixtures.mkdir(parents=True, exist_ok=True)
output.write_bytes(pdf)
(fixtures / output.name).write_bytes(pdf)

# Deliberately weak encryption ONLY for a deterministic locked-document test;
# this is not the application's storage encryption or a user password.
locked = PdfWriter()
locked.clone_document_from_reader(reader)
identity = ByteStringObject(hashlib.sha256(b"PasteRS locked fixture").digest()[:16])
locked._ID = ArrayObject([identity, identity])
locked.encrypt("synthetic-only-password", algorithm="RC4-128")
locked_bytes = io.BytesIO()
locked.write(locked_bytes)
(fixtures / "native-preview-locked.pdf").write_bytes(locked_bytes.getvalue())
locked_reader = PdfReader(io.BytesIO(locked_bytes.getvalue()))
assert locked_reader.is_encrypted and not locked_reader.decrypt("")
assert locked_reader.decrypt("synthetic-only-password")
assert len(locked_reader.pages) == 3
print(json.dumps({"result": "passed", "pages": 3, "externalActions": False,
                  "pdfSha256": hashlib.sha256(pdf).hexdigest(),
                  "lockedSha256": hashlib.sha256(locked_bytes.getvalue()).hexdigest()}))
