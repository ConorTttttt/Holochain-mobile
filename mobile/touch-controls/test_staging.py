"""Unit tests for staging original game assets without altering the source."""
import subprocess
import sys
import tempfile
from pathlib import Path
import unittest

SCRIPT = Path(__file__).resolve().parents[2] / "scripts" / "stage-turbo-kart-touch-game.py"


class StageGameTests(unittest.TestCase):
    def test_copy_inject_and_allowlist(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            source = root / "game-original"
            (source / "src").mkdir(parents=True)
            (source / "index.html").write_text(
                '<html><head></head><body><script type="module" src="./src/index.js"></script></body></html>',
                encoding="utf-8",
            )
            (source / "src" / "index.js").write_text('console.log("game")', encoding="utf-8")
            (source / "private-notes.txt").write_text("do not ship", encoding="utf-8")
            target = root / "apk" / "game"
            result = subprocess.run(
                [
                    sys.executable, str(SCRIPT),
                    "--source", str(source), "--dest", str(target),
                ],
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertTrue((target / "src" / "index.js").exists())
            self.assertFalse((target / "private-notes.txt").exists())
            self.assertTrue((target / "mobile-touch.mjs").exists())
            content = (target / "index.html").read_text(encoding="utf-8")
            self.assertIn('meta name="viewport"', content)
            self.assertEqual(content.count("mobile-touch.mjs"), 1)
            self.assertNotIn("mobile-touch.mjs", (source / "index.html").read_text())

    def test_reject_missing_game_source(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            root.joinpath("index.html").write_text("<html><body></body></html>")
            result = subprocess.run(
                [sys.executable, str(SCRIPT), "--source", str(root), "--dest", str(root / "assets")],
                capture_output=True,
                text=True,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("index.html and src/", result.stderr)


if __name__ == "__main__":
    unittest.main()
