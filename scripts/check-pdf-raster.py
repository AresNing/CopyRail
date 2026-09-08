"""Compare synthetic PDF raster geometry with independent Poppler output.

Uses bundled Pillow/pypdf. This is renderer QA, not Paste pixel acceptance.
"""
import json
from pathlib import Path

from PIL import Image
from pypdf import PdfReader

ROOT = Path(__file__).resolve().parent.parent
OUTPUT = ROOT / "target/pdf-preview-verification"
checks = []
for name in ("portrait", "rotated", "cropped"):
    document = PdfReader(ROOT / f"tmp/pdfs/synthetic-{name}.pdf")
    assert len(document.pages) == 2
    assert "First page" in document.pages[0].extract_text()
    actual = Image.open(OUTPUT / f"{name}.png").convert("RGBA")
    reference = Image.open(OUTPUT / f"reference-{name}.png").convert("RGBA")
    assert actual.size == reference.size
    assert actual.getchannel("A").getextrema() == (255, 255)
    actual_pixels = list(actual.get_flattened_data())
    reference_pixels = list(reference.get_flattened_data())
    overlaps = {}
    for label, channel in (("red", 0), ("blue", 2)):
        def selected(pixel):
            return pixel[channel] > 240 and all(pixel[c] < 10 for c in range(3) if c != channel)
        left = {i for i, pixel in enumerate(actual_pixels) if selected(pixel)}
        right = {i for i, pixel in enumerate(reference_pixels) if selected(pixel)}
        overlap = len(left & right) / len(left | right)
        assert overlap > 0.99, (name, label, overlap)
        overlaps[label] = overlap
    checks.append({"name": name, "pages": 2, "pixelSize": actual.size, "colorMaskIoU": overlaps})
acceptance = Image.open(OUTPUT / "native-acceptance.png").convert("RGBA")
reference = Image.open(ROOT / "tmp/pdfs/native-preview-acceptance/page-1.png").convert("RGBA")
assert acceptance.size == reference.size == (675, 900)
assert acceptance.getchannel("A").getextrema() == (255, 255)
color_overlaps = {}
for label, color in (("red", (213, 71, 67)), ("blue", (19, 92, 197)), ("green", (39, 130, 90))):
    def matching(pixel):
        return all(abs(pixel[channel] - color[channel]) <= 2 for channel in range(3))
    actual_mask = {i for i, pixel in enumerate(acceptance.get_flattened_data()) if matching(pixel)}
    reference_mask = {i for i, pixel in enumerate(reference.get_flattened_data()) if matching(pixel)}
    assert len(actual_mask) > 1_000 and len(reference_mask) > 1_000
    overlap = len(actual_mask & reference_mask) / len(actual_mask | reference_mask)
    color_overlaps[label] = overlap
acceptance_matches = all(value > 0.99 for value in color_overlaps.values())
checks.append({"name": "native-acceptance", "pages": 3, "pixelSize": acceptance.size,
               "firstPageColorMaskIoU": color_overlaps, "requiredIoU": 0.99,
               "result": "passed" if acceptance_matches else "failed",
               "redTopBoundarySamples": [
                   {"xy": [450, y], "coreGraphics": acceptance.getpixel((450, y)),
                    "poppler": reference.getpixel((450, y))} for y in (522, 523, 524)
               ]})
report = {"result": "passed" if acceptance_matches else "failed", "nativeWindow": False, "checks": checks,
          "limitation": "Strict opaque masks include vector and text edge rasterization differences. This is not a comparison against Paste or native window acceptance."}
(OUTPUT / "independent-raster-report.json").write_text(json.dumps(report, indent=2) + "\n")
print(json.dumps(report))
raise SystemExit(0 if acceptance_matches else 1)
