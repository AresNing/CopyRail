#!/usr/bin/env python3
"""Package an already checked release app; never publish, install, or read user data."""
import hashlib
import json
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile
import zipfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
VERSION = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
APP = ROOT / "target/release/bundle/macos/CopyRail.app"
DEST = ROOT / "output" / f"CopyRail-{VERSION}"
sha = lambda p: hashlib.sha256(p.read_bytes()).hexdigest()

def run(*args, **kwargs):
    return subprocess.check_output(args, text=True, **kwargs).strip()

info = plistlib.loads((APP / "Contents/Info.plist").read_bytes())
assert info["CFBundleIdentifier"] == "io.pasters.desktop"
assert info["CFBundleShortVersionString"] == VERSION
assert info["LSMinimumSystemVersion"] == "13.0"
binary = APP / "Contents/MacOS/pasters-desktop"
build = json.loads(run(str(binary), "--build-info"))
assert build["version"] == VERSION and not build["debugBuild"] and not build["cloudTransportCompiled"]
assert run("lipo", "-archs", str(binary)) == "arm64"
assert not DEST.exists(), f"Refusing to overwrite an existing delivery: {DEST}"
run("codesign", "--force", "--deep", "--sign", "-", "--entitlements", str(ROOT / "apps/desktop/src-tauri/CopyRail.release.entitlements"), str(APP))
run("codesign", "--verify", "--deep", "--strict", str(APP))
DEST.mkdir(parents=True)
shutil.copytree(APP, DEST / "CopyRail.app")
downloads = DEST / "downloads"
downloads.mkdir()
assets = {p.name: sha(p) for p in (ROOT / "apps/desktop/dist").iterdir() if p.suffix in (".html", ".css", ".js", ".wasm")}
manifest = {"formatVersion": 1, "deliveryVersion": VERSION, "app": "CopyRail.app", "buildInfo": build,
            "binary": "CopyRail.app/Contents/MacOS/pasters-desktop", "binaryBytes": binary.stat().st_size,
            "binarySha256": sha(binary), "frontendAssetSha256": assets,
            "signature": "ad-hoc; no Developer ID or Apple notarization", "architecture": "arm64",
            "minimumMacOS": "13.0", "installed": False, "published": False}
(DEST / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
# Only these reviewed public documents enter downloads; no output/ evidence,
# databases, traces, workspace configuration, or development logs are included.
with tempfile.TemporaryDirectory(prefix="copyrail-release-package-") as temporary:
    stage = Path(temporary) / "payload"
    stage.mkdir()
    shutil.copytree(APP, stage / "CopyRail.app")
    for name in ["LICENSE", "NOTICE", "THIRD_PARTY_NOTICES.md"]:
        shutil.copy2(ROOT / name, stage / name)
    shutil.copy2(ROOT / "docs/installation.md", stage / "INSTALL.md")
    (stage / "docs").mkdir()
    shutil.copy2(ROOT / "docs/dependencies.md", stage / "docs/dependencies.md")
    shutil.copytree(ROOT / "licenses", stage / "licenses")
    metadata = json.loads(run("cargo", "metadata", "--locked", "--format-version", "1", cwd=ROOT))
    lock = tomllib.loads((ROOT / "Cargo.lock").read_text())
    checksums = {(p["name"], p["version"]): p.get("checksum") for p in lock["package"]}
    inventory = []
    mpl_sources = downloads / f"CopyRail_{VERSION}_third-party-sources.zip"
    with zipfile.ZipFile(mpl_sources, "w", zipfile.ZIP_DEFLATED) as source_zip:
        for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
            if not (package.get("source") or "").startswith("registry+"):
                continue
            directory = Path(package["manifest_path"]).parent
            key = f"{package['name']}-{package['version']}"
            notice_files = [p for p in directory.rglob("*") if p.is_file() and not p.is_symlink()
                            and p.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "COPYRIGHT", "NOTICE", "UNLICENSE"))]
            # Some upstream crates carry the notice in their README.
            notice_files += [p for p in directory.glob("README*") if p.is_file()]
            explicit = package.get("license_file")
            if explicit:
                notice_files.append(directory / explicit)
            for notice in set(notice_files):
                relative = notice.relative_to(directory)
                assert ".." not in relative.parts
                target = stage / "licenses/dependencies" / key / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(notice, target)
            inventory.append({"name": package["name"], "version": package["version"], "license": package.get("license"),
                              "authors": package.get("authors", []), "repository": package.get("repository"),
                              "source": f"https://crates.io/api/v1/crates/{package['name']}/{package['version']}/download"})
            if "MPL-2.0" in (package.get("license") or ""):
                archive_source = directory.parent.parent.parent / "cache" / directory.parent.name / f"{key}.crate"
                assert sha(archive_source) == checksums[(package["name"], package["version"])], key
                source_zip.write(archive_source, f"mpl-sources/{key}.crate")
        source_zip.writestr("README.txt", "Exact, unmodified upstream MPL-2.0 crate source archives from Cargo.lock. Each .crate is a gzip-compressed tar archive. Original license and notice files are inside. CopyRail uses these packages without local modifications.\n")
    (stage / "licenses/dependency-inventory.json").write_text(json.dumps(inventory, indent=2) + "\n")
    archive = downloads / f"CopyRail_{VERSION}_aarch64.zip"
    run("ditto", "-c", "-k", "--sequesterRsrc", str(stage), str(archive))
    (stage / "Applications").symlink_to("/Applications")
    dmg = downloads / f"CopyRail_{VERSION}_aarch64.dmg"
    run("hdiutil", "create", "-volname", f"CopyRail {VERSION}", "-srcfolder", str(stage), "-format", "UDZO", "-ov", str(dmg))
    run("hdiutil", "verify", str(dmg))
public = {"version": VERSION, "architecture": "arm64", "minimumMacOS": "13.0",
          "appIdentifier": "io.pasters.desktop", "binarySha256": sha(binary),
          "developerIDSigned": False, "notarized": False,
          "assets": {p.name: {"sha256": sha(p), "bytes": p.stat().st_size} for p in (archive, dmg, mpl_sources)}}
(downloads / "RELEASE_MANIFEST.json").write_text(json.dumps(public, indent=2) + "\n")
(downloads / "SHA256SUMS").write_text("".join(f"{sha(p)}  {p.name}\n" for p in sorted(downloads.iterdir()) if p.name != "SHA256SUMS"))
print(f"Packaged {VERSION}: {downloads}")
