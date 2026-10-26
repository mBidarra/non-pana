"""Transcrição de vídeos do YouTube para o novod: POST /transcript {"url", "langs"?} devolve o texto falado.

Só escuta em 127.0.0.1. Tenta a youtube-transcript-api e, se ela falhar, o yt-dlp. Guarda o resultado em cache por vídeo.
Uso: python server.py [--port 8790]
"""
import argparse
import hashlib
import html
import json
import os
import re
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

CACHE = Path(os.environ.get("NOVO_YT_CACHE", Path.home() / "Library" / "Caches" / "novo-yt-transcript"))
DEFAULT_LANGS = ["pt", "pt-BR", "en", "en-US", "es"]
ID_RE = re.compile(r"^[A-Za-z0-9_-]{11}$")
MAX_BODY = 16 * 1024
_lock = threading.Lock()


class Unavailable(Exception):
    """O vídeo existe, mas não tem legenda que dê para ler."""


def video_id(url: str) -> str | None:
    """Aceita watch?v=, youtu.be/, /shorts/, /live/, /embed/ e o id puro."""
    url = (url or "").strip()
    if ID_RE.match(url):
        return url
    try:
        u = urlparse(url)
    except ValueError:
        return None
    host = (u.hostname or "").lower().removeprefix("www.").removeprefix("m.").removeprefix("music.")
    if host == "youtu.be":
        cand = u.path.lstrip("/").split("/")[0]
    elif host in ("youtube.com", "youtube-nocookie.com"):
        if u.path == "/watch":
            cand = parse_qs(u.query).get("v", [""])[0]
        else:
            parts = [p for p in u.path.split("/") if p]
            cand = parts[1] if len(parts) >= 2 and parts[0] in ("shorts", "live", "embed", "v") else ""
    else:
        return None
    return cand if ID_RE.match(cand) else None


def via_transcript_api(vid: str, langs: list[str]) -> dict:
    from youtube_transcript_api import YouTubeTranscriptApi
    from youtube_transcript_api._errors import NoTranscriptFound, TranscriptsDisabled, VideoUnavailable

    try:
        tl = YouTubeTranscriptApi().list(vid)
    except (TranscriptsDisabled, VideoUnavailable) as e:
        raise Unavailable(type(e).__name__) from e
    t = None
    # legenda feita por pessoa vence a automática; fora da lista, vale a primeira que houver
    for find in (tl.find_manually_created_transcript, tl.find_generated_transcript):
        try:
            t = find(langs)
            break
        except NoTranscriptFound:
            continue
    if t is None:
        t = next(iter(tl), None)
    if t is None:
        raise Unavailable("sem legendas")
    parts = [s.text.replace("\n", " ").strip() for s in t.fetch()]
    return {"language": t.language_code, "is_generated": t.is_generated, "source": "youtube-transcript-api", "text": " ".join(p for p in parts if p)}


def vtt_text(vtt: str) -> str:
    """VTT → texto corrido. Legendas automáticas repetem a linha anterior a cada cue: só entra o que é novo."""
    out: list[str] = []
    for line in vtt.splitlines():
        line = line.strip()
        if not line or "-->" in line or line.startswith(("WEBVTT", "Kind:", "Language:", "NOTE")) or line.isdigit():
            continue
        line = html.unescape(re.sub(r"<[^>]+>", "", line)).strip()
        if line and (not out or out[-1] != line):
            out.append(line)
    return " ".join(out)


def via_ytdlp(vid: str, langs: list[str]) -> dict:
    import yt_dlp

    with tempfile.TemporaryDirectory() as tmp:
        opts = {
            "skip_download": True,
            "writesubtitles": True,
            "writeautomaticsub": True,
            "subtitleslangs": [f"{l}.*" for l in langs] + langs,
            "subtitlesformat": "vtt",
            "outtmpl": os.path.join(tmp, "%(id)s.%(ext)s"),
            "quiet": True,
            "noprogress": True,
            "no_warnings": True,
            "ignoreerrors": True,
        }
        with yt_dlp.YoutubeDL(opts) as y:
            info = y.extract_info(f"https://www.youtube.com/watch?v={vid}", download=True) or {}
        files = sorted(Path(tmp).glob(f"{vid}.*.vtt"))
        if not files:
            raise Unavailable("yt-dlp não baixou legenda")
        manual = set((info.get("subtitles") or {}).keys())
        # prefere a legenda manual na ordem de langs; depois qualquer uma
        def rank(p: Path):
            lang = p.name[len(vid) + 1 : -len(".vtt")]
            base = lang.split("-")[0]
            pos = next((i for i, l in enumerate(langs) if lang == l or base == l.split("-")[0]), len(langs))
            return (lang not in manual, pos, len(lang))
        best = min(files, key=rank)
        lang = best.name[len(vid) + 1 : -len(".vtt")]
        text = vtt_text(best.read_text(encoding="utf-8", errors="replace"))
        if not text:
            raise Unavailable("legenda vazia")
        return {"language": lang, "is_generated": lang not in manual, "source": "yt-dlp", "text": text, "title": info.get("title")}


def transcript(url: str, langs: list[str]) -> dict:
    vid = video_id(url)
    if not vid:
        raise ValueError("não é um link de vídeo do YouTube")
    key = hashlib.sha1(f"{vid}|{','.join(langs)}".encode()).hexdigest()[:16]
    cached = CACHE / f"{vid}-{key}.json"
    if cached.exists():
        return {**json.loads(cached.read_text()), "cached": True}
    errors = []
    res = None
    for fn in (via_transcript_api, via_ytdlp):
        try:
            res = fn(vid, langs)
            break
        except Exception as e:  # noqa: BLE001 — a próxima via tenta; o motivo volta no erro final
            errors.append(f"{fn.__name__}: {type(e).__name__}: {e}"[:300])
    if res is None:
        raise Unavailable("; ".join(errors))
    res = {"video_id": vid, "url": f"https://www.youtube.com/watch?v={vid}", "words": len(res["text"].split()), **res}
    with _lock:
        CACHE.mkdir(parents=True, exist_ok=True)
        tmp = cached.with_suffix(".tmp")
        tmp.write_text(json.dumps(res, ensure_ascii=False))
        tmp.replace(cached)
    return {**res, "cached": False}


class Handler(BaseHTTPRequestHandler):
    server_version = "novo-yt-transcript/0.1"

    def _send(self, code: int, body: dict):
        data = json.dumps(body, ensure_ascii=False).encode()
        self.send_response(code)
        self.send_header("content-type", "application/json; charset=utf-8")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path == "/health":
            return self._send(200, {"ok": True})
        self._send(404, {"error": "use POST /transcript"})

    def do_POST(self):
        if self.path != "/transcript":
            return self._send(404, {"error": "use POST /transcript"})
        n = int(self.headers.get("content-length") or 0)
        if n <= 0 or n > MAX_BODY:
            return self._send(400, {"error": "corpo JSON ausente ou grande demais"})
        try:
            req = json.loads(self.rfile.read(n))
            langs = [str(l) for l in (req.get("langs") or DEFAULT_LANGS)][:10]
            return self._send(200, transcript(str(req.get("url", "")), langs))
        except (ValueError, json.JSONDecodeError) as e:
            return self._send(400, {"error": str(e)})
        except Unavailable as e:
            return self._send(404, {"error": f"transcrição indisponível: {e}"})
        except Exception as e:  # noqa: BLE001
            return self._send(502, {"error": f"{type(e).__name__}: {e}"[:500]})

    def log_message(self, fmt, *args):
        sys.stderr.write("%s %s\n" % (self.log_date_time_string(), fmt % args))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=int(os.environ.get("NOVO_YT_PORT", 8790)))
    a = ap.parse_args()
    srv = ThreadingHTTPServer(("127.0.0.1", a.port), Handler)
    print(f"novo-yt-transcript em http://127.0.0.1:{a.port}", flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
