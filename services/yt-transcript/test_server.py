"""python -m unittest test_server (dentro de services/yt-transcript): só o que não precisa de rede."""
import unittest

from server import video_id, vtt_text


class Ids(unittest.TestCase):
    def test_formats(self):
        for u in [
            "https://www.youtube.com/watch?v=A0kTUP7fWtk",
            "https://youtube.com/watch?v=A0kTUP7fWtk&t=42s",
            "https://m.youtube.com/watch?v=A0kTUP7fWtk",
            "https://youtu.be/A0kTUP7fWtk?si=x",
            "https://www.youtube.com/shorts/A0kTUP7fWtk",
            "https://www.youtube.com/live/A0kTUP7fWtk",
            "https://www.youtube.com/embed/A0kTUP7fWtk",
            "A0kTUP7fWtk",
        ]:
            self.assertEqual(video_id(u), "A0kTUP7fWtk", u)

    def test_rejects(self):
        for u in ["https://vimeo.com/123", "https://www.youtube.com/channel/abc", "https://evil.com/watch?v=A0kTUP7fWtk", "", "nada"]:
            self.assertIsNone(video_id(u), u)


class Vtt(unittest.TestCase):
    def test_rolling_auto_captions(self):
        vtt = "WEBVTT\nKind: captions\nLanguage: en\n\n00:00:00.000 --> 00:00:02.000\nhello <c>world</c>\n\n00:00:02.000 --> 00:00:04.000\nhello world\nthis &amp; that\n"
        self.assertEqual(vtt_text(vtt), "hello world this & that")


if __name__ == "__main__":
    unittest.main()
