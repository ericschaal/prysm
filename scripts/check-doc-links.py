#!/usr/bin/env python3
"""Check public Markdown links against a clean checkout; optionally check HTTP URLs."""
import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from html import unescape
import ipaddress
from pathlib import Path
import re
import subprocess
import time
import unicodedata
from urllib.parse import unquote, urlsplit
from urllib.request import Request, urlopen

ROOT = Path(__file__).resolve().parents[1]


def prose(text):
    # Fences may contain TOML arrays and example URLs, which are not Markdown links.
    text = re.sub(r"^(`{3,}|~{3,})[^\n]*\n.*?^\1\s*$", "", text, flags=re.M | re.S)
    return text


def links(text):
    text = re.sub(r"`+[^`\n]*`+", "", prose(text))
    destinations = re.findall(r'!?\[[^\]\n]*\]\(\s*(?:<([^>]+)>|([^\s)]+))(?:\s+"[^"\n]*")?\s*\)', text)
    result = [a or b for a, b in destinations]
    definitions = dict(re.findall(r"^\s*\[([^\]]+)\]:\s*<?([^\s>]+)>?", text, re.M))
    for label, ref in re.findall(r"!?\[([^\]\n]+)\]\[([^\]\n]*)\]", text):
        result.append(definitions.get(ref or label, "missing-reference:" + (ref or label)))
    result.extend(definitions.values())
    result.extend(re.findall(r'''(?:href|src)\s*=\s*["']([^"']+)["']''', text))
    result.extend(re.findall(r"<(https?://[^>\s]+)>", text))
    return [unescape(value) for value in result]


def anchors(text):
    text = re.sub(r"`+([^`\n]*)`+", r"\1", prose(text))
    result = set(re.findall(r'''(?:id|name)\s*=\s*["']([^"']+)["']''', text))
    counts = Counter()
    for heading in re.findall(r"^#{1,6}\s+(.+?)\s*#*\s*$", text, re.M):
        heading = unescape(re.sub(r"<[^>]+>", "", heading)).lower()
        slug = "".join(c for c in heading if not unicodedata.category(c).startswith(("P", "S")) or c in "-_")
        slug = slug.replace(" ", "-")
        count = counts[slug]
        counts[slug] += 1
        result.add(slug + ("-" + str(count) if count else ""))
    return result


def local_error(source, destination, public_files, root):
    url = urlsplit(destination)
    if url.scheme == "missing-reference":
        return "undefined reference: " + url.path
    if url.scheme or url.netloc:
        return None
    target = (source.parent / unquote(url.path)).resolve() if url.path else source
    if not target.is_relative_to(root):
        return "target outside repository"
    if not target.exists():
        return "missing target"
    if target not in public_files and not (target.is_dir() and any(target in p.parents for p in public_files)):
        return "target is ignored or untracked in a clean checkout"
    if url.fragment and target.suffix.lower() == ".md" and unquote(url.fragment) not in anchors(target.read_text()):
        return "missing heading/anchor"
    return None


def runtime_url(url):
    host = urlsplit(url).hostname or ""
    if host == "localhost" or host.endswith(".local"):
        return True
    try:
        return not ipaddress.ip_address(host).is_global
    except ValueError:
        return False


def external_error(url):
    for attempt in range(3):
        try:
            request = Request(url, headers={"User-Agent": "Mozilla/5.0 (Prysm documentation link check)"})
            with urlopen(request, timeout=20) as response:
                if response.status < 400:
                    return None
        except Exception as error:
            if attempt == 2:
                return str(error)
            time.sleep(attempt + 1)
    return "HTTP request failed"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--external", action="store_true", help="also GET public HTTP(S) URLs, with retries")
    args = parser.parse_args()
    names = subprocess.check_output(["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"], cwd=ROOT).decode().split("\0")
    public = {ROOT / name for name in names if name and (ROOT / name).exists()}
    errors, urls, count = [], set(), 0
    markdown = sorted(p for p in public if p.suffix.lower() == ".md")
    for source in markdown:
        for destination in links(source.read_text()):
            count += 1
            error = local_error(source, destination, public, ROOT)
            if error:
                errors.append(f"{source.relative_to(ROOT)}: {destination}: {error}")
            if destination.startswith(("http://", "https://")) and not runtime_url(destination):
                urls.add(destination)
    if args.external:
        with ThreadPoolExecutor(max_workers=8) as pool:
            for url, error in zip(sorted(urls), pool.map(external_error, sorted(urls))):
                if error:
                    errors.append(f"{url}: {error}")
    for error in errors:
        print(error)
    print(f"Checked {count} links in {len(markdown)} Markdown files; {len(errors)} errors.")
    print(f"Public HTTP(S) URLs: {len(urls)} ({'checked' if args.external else 'use --external to check'}). Runtime LAN/localhost URLs are excluded.")
    return bool(errors)


if __name__ == "__main__":
    raise SystemExit(main())
