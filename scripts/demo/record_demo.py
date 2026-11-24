"""Record the FileCraft desktop demo from the real Windows application.

The script starts a built `filecraft-desktop.exe` with WebView2 remote debugging,
drives the interface with Playwright, captures frames through the Chrome
DevTools screencast API and renders an MP4 and a GIF with FFmpeg.

Only the operating-system file pickers are answered by the script, because
native dialogs live outside the web view. Inspection, processing and
verification run in the FileCraft engine exactly as they do for a user. The
long encoding stretch is fast-forwarded and labelled as such in the video.

    python scripts/demo/record_demo.py --app <filecraft-desktop.exe> \
        --inputs <folder with the files from make_inputs.py> \
        --output <empty folder for FileCraft results> --work <scratch folder>

Requires Windows with WebView2, Python 3.10+, Playwright for Python and FFmpeg.
"""

from __future__ import annotations

import argparse
import asyncio
import base64
import json
import os
import subprocess
import time
import urllib.request
from pathlib import Path

from playwright.async_api import Locator, Page, async_playwright

PORT = 9333
WIDTH, HEIGHT = 1280, 800
INPUTS = ("IMG_4821.HEIC", "signed-forms-scan.pdf", "IMG_4822.MOV")
WEBVIEW_ARGS = " ".join([
    f"--remote-debugging-port={PORT}",
    "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,CalculateNativeWinOcclusion",
    "--disable-backgrounding-occluded-windows",
    "--disable-renderer-backgrounding",
    "--disable-background-timer-throttling",
])

OVERLAY = r"""
({ files, folder }) => {
  const original = window.fetch.bind(window)
  window.fetch = async (input, init) => {
    const url = typeof input === 'string' ? input : input.url
    if (url.includes('plugin%3Adialog%7Copen')) {
      const body = init && typeof init.body === 'string' ? JSON.parse(init.body) : {}
      await new Promise(resolve => setTimeout(resolve, 180))
      const value = body.options && body.options.directory ? folder : files
      return new Response(JSON.stringify(value), {
        status: 200, headers: { 'Content-Type': 'application/json', 'Tauri-Response': 'ok' },
      })
    }
    return original(input, init)
  }

  const style = document.createElement('style')
  style.textContent = `
    html { scrollbar-width: none; }
    #demo-layer { position: fixed; inset: 0; pointer-events: none; z-index: 2147483647; }
    #demo-cursor { position: absolute; left: 0; top: 0; width: 26px; height: 32px; transform: translate(640px, 470px);
      filter: drop-shadow(0 2px 3px #0005); transition: opacity .3s; }
    #demo-ripple { position: absolute; width: 38px; height: 38px; margin: -19px 0 0 -19px; border-radius: 50%;
      border: 3px solid #bd4626; opacity: 0; }
    #demo-ripple.on { animation: demo-ripple .5s ease-out; }
    @keyframes demo-ripple { from { opacity: .9; transform: scale(.3); } to { opacity: 0; transform: scale(1.5); } }
    #demo-caption { position: absolute; left: 50%; bottom: 30px; white-space: nowrap; transform: translate(-50%, 12px);
      padding: 15px 26px; border-radius: 14px; background: #262622ee; color: #fffefb; opacity: 0;
      font: 600 21px/1.35 'Manrope Variable', sans-serif; letter-spacing: -.005em; text-align: center;
      box-shadow: 0 12px 34px #1b130c40; transition: opacity .35s, transform .35s; }
    #demo-caption.on { opacity: 1; transform: translate(-50%, 0); }
    #demo-caption b { color: #f2a07f; font-weight: 800; }
    #demo-badge { position: absolute; left: 50%; top: 22px; transform: translateX(-50%); padding: 7px 14px;
      border-radius: 999px; background: #bd4626; color: #fffefb; opacity: 0; transition: opacity .3s;
      font: 800 13px/1 'Manrope Variable', sans-serif; letter-spacing: .12em; }
    #demo-badge.on { opacity: 1; }
    #demo-ring { position: absolute; border: 3px solid #bd4626; border-radius: 12px; opacity: 0;
      box-shadow: 0 0 0 6px #bd462626; transition: opacity .3s; }
    #demo-ring.on { opacity: 1; }
    #demo-card { position: absolute; inset: 0; display: flex; flex-direction: column; justify-content: center;
      padding: 0 120px; background: radial-gradient(circle at 78% 30%, #bd462640, transparent 46%), #262622;
      color: #fffefb; opacity: 0; transition: opacity .55s; }
    #demo-card.on { opacity: 1; }
    #demo-card .tag { font: 800 15px/1 'Manrope Variable', sans-serif; letter-spacing: .2em; color: #f2a07f; }
    #demo-card h1 { margin: 26px 0 0; max-width: 980px; font: 700 62px/1.04 'Space Grotesk Variable', sans-serif;
      letter-spacing: -.035em; }
    #demo-card p { margin: 26px 0 0; max-width: 860px; font: 500 24px/1.45 'Manrope Variable', sans-serif;
      color: #d9d4c7; }
    #demo-card code { display: inline-block; margin-top: 34px; padding: 12px 18px; border-radius: 10px;
      background: #fffefb14; font: 700 22px/1 'Space Grotesk Variable', monospace; color: #fffefb; }
  `
  document.head.appendChild(style)

  const layer = document.createElement('div')
  layer.id = 'demo-layer'
  layer.innerHTML = `
    <div id="demo-ring"></div><div id="demo-badge">FAST-FORWARD ▶▶</div><div id="demo-caption"></div>
    <div id="demo-ripple"></div>
    <svg id="demo-cursor" viewBox="0 0 26 32"><path d="M3 2 L3 26 L9.5 20 L14 30 L18 28 L13.6 18.6 L22 18.4 Z"
      fill="#fff" stroke="#151512" stroke-width="2" stroke-linejoin="round"/></svg>
    <div id="demo-card"></div>`
  document.body.appendChild(layer)
  const $ = id => document.getElementById(id)
  const cursor = $('demo-cursor')
  let point = [640, 470]
  document.addEventListener('mousemove', event => {
    point = [event.clientX, event.clientY]
    cursor.style.transform = `translate(${event.clientX - 3}px, ${event.clientY - 2}px)`
  }, true)

  window.__demo = {
    caption(html) {
      const node = $('demo-caption')
      if (!html) { node.classList.remove('on'); return }
      node.classList.remove('on')
      setTimeout(() => { node.innerHTML = html; node.classList.add('on') }, node.textContent ? 260 : 0)
    },
    badge(on) { $('demo-badge').classList.toggle('on', on) },
    press() {
      const ripple = $('demo-ripple')
      ripple.style.left = point[0] + 'px'; ripple.style.top = point[1] + 'px'
      ripple.classList.remove('on'); void ripple.offsetWidth; ripple.classList.add('on')
    },
    ring(selector) {
      const ring = $('demo-ring')
      const target = selector && document.querySelector(selector)
      if (!target) { ring.classList.remove('on'); return }
      const box = target.getBoundingClientRect()
      Object.assign(ring.style, { left: box.left - 7 + 'px', top: box.top - 7 + 'px',
        width: box.width + 14 + 'px', height: box.height + 14 + 'px' })
      ring.classList.add('on')
    },
    card(html) {
      const card = $('demo-card')
      if (html) card.innerHTML = html
      card.classList.toggle('on', Boolean(html))
      cursor.style.opacity = html ? '0' : '1'
    },
  }
}
"""


class Recorder:
    def __init__(self, work: Path) -> None:
        self.frames_dir = work / "frames"
        self.frames_dir.mkdir(parents=True, exist_ok=True)
        self.frames: list[dict] = []
        self.marks: dict[str, float] = {}
        self.session = None

    async def start(self, page: Page) -> None:
        self.session = await page.context.new_cdp_session(page)
        self.session.on("Page.screencastFrame", lambda params: asyncio.ensure_future(self.frame(params)))
        await self.session.send("Page.startScreencast", {
            "format": "jpeg", "quality": 94, "maxWidth": 1920, "maxHeight": 1200, "everyNthFrame": 1,
        })

    async def frame(self, params: dict) -> None:
        name = f"{len(self.frames):05d}.jpg"
        (self.frames_dir / name).write_bytes(base64.b64decode(params["data"]))
        self.frames.append({"file": name, "t": params["metadata"]["timestamp"]})
        try:
            await self.session.send("Page.screencastFrameAck", {"sessionId": params["sessionId"]})
        except Exception:
            pass

    def mark(self, name: str) -> None:
        self.marks[name] = time.time()

    async def stop(self, work: Path) -> None:
        await self.session.send("Page.stopScreencast")
        await asyncio.sleep(0.3)
        (work / "frames.json").write_text(json.dumps(self.frames))
        (work / "marks.json").write_text(json.dumps(self.marks, indent=2))


class Pointer:
    def __init__(self, page: Page) -> None:
        self.page = page
        self.x, self.y = 640.0, 470.0

    async def glide(self, target: Locator, seconds: float = 0.9, dx: float = 0, dy: float = 0) -> None:
        box = await target.bounding_box()
        if box is None:
            raise RuntimeError(f"{target} is not visible")
        x, y = box["x"] + box["width"] / 2 + dx, box["y"] + box["height"] / 2 + dy
        steps = max(12, int(seconds * 60))
        start_x, start_y = self.x, self.y
        for step in range(1, steps + 1):
            t = step / steps
            ease = 4 * t ** 3 if t < 0.5 else 1 - (-2 * t + 2) ** 3 / 2
            await self.page.mouse.move(start_x + (x - start_x) * ease, start_y + (y - start_y) * ease)
            await asyncio.sleep(seconds / steps)
        self.x, self.y = x, y

    async def click(self) -> None:
        await self.page.evaluate("() => window.__demo.press()")
        await self.page.mouse.down()
        await asyncio.sleep(0.09)
        await self.page.mouse.up()


async def demo(
    call, page: Page, recorder: Recorder, pointer: Pointer, repo: str, limit_mb: str
) -> None:
    await call("card", """<div class="tag">FILECRAFT · FREE AND OPEN SOURCE</div>
      <h1>Fit a whole application package under one size limit.</h1>
      <p>iPhone photos, scanned PDFs and HDR videos, processed and verified on your own device.</p>""")
    await asyncio.sleep(0.8)
    recorder.mark("start")
    await asyncio.sleep(3.0)
    await call("card", None)
    await asyncio.sleep(0.7)

    await call("caption", f"The visa portal accepts <b>{limit_mb} MB</b> in total.")
    await asyncio.sleep(1.2)
    await pointer.glide(page.get_by_role("button", name="Browse files"), 1.1)
    await asyncio.sleep(0.25)
    await pointer.click()
    await page.locator(".file-list > li").nth(len(INPUTS) - 1).wait_for(timeout=60_000)
    await asyncio.sleep(0.5)
    total = (await page.locator(".file-count").inner_text()).split("·")[-1].strip()
    await call("caption", f"An iPhone photo, a scanned form and an HDR video: <b>{total}</b>.")
    await pointer.glide(page.locator(".file-list > li").nth(2), 1.2, dx=-120)
    await asyncio.sleep(2.0)

    await call("ring", ".goal-panel .formats")
    await call("caption", "<b>Mixed batch</b> gives photos, PDFs and videos one shared limit.")
    await pointer.glide(page.locator(".goal-panel .formats button").first, 1.0)
    await asyncio.sleep(1.6)
    await call("ring", None)
    field = page.locator(".goal-panel .fields .field input").first
    await pointer.glide(field, 0.9, dx=-30)
    await pointer.click()
    await page.keyboard.press("Control+A")
    await asyncio.sleep(0.2)
    await page.keyboard.type(limit_mb, delay=160)
    await call("ring", ".goal-panel .fields .field")
    await call("caption", f"Set the limit to <b>{limit_mb} MB</b>.")
    await asyncio.sleep(1.8)
    await call("ring", None)

    await pointer.glide(page.locator("button.run"), 0.9)
    await asyncio.sleep(0.2)
    await pointer.click()
    card = page.locator("article.job-card")
    await card.wait_for(timeout=30_000)
    await call("caption", "FileCraft encodes candidates and verifies every output.")
    await page.evaluate("() => window.scrollTo({ top: document.documentElement.scrollHeight, behavior: 'smooth' })")
    await asyncio.sleep(1.6)
    recorder.mark("fast_start")
    await call("badge", True)
    finished = page.locator("article.job-card .goal-result, article.job-card .warning")
    await finished.first.wait_for(timeout=600_000)
    await asyncio.sleep(0.3)
    recorder.mark("fast_end")
    await call("badge", False)
    heading = await card.locator("h3").inner_text()
    if heading != "Verified output":
        raise RuntimeError(f"The demo job did not complete: {heading}")

    await page.evaluate("""() => document.querySelector('article.job-card')
      .scrollIntoView({ behavior: 'smooth', block: 'start' })""")
    await asyncio.sleep(0.9)
    result = await card.locator(".goal-result").inner_text()
    await call("caption", f"<b>{result.strip()}</b>. Each file passed its checks.")
    await pointer.glide(card.locator(".stats"), 1.0, dy=10)
    recorder.mark("result")
    await asyncio.sleep(2.6)

    for name, caption in (
        ("IMG_4821", "The iPhone photo becomes a JPEG. Its EXIF, including the GPS position, is removed."),
        ("signed-forms-scan", "Scanned pages are downsampled to 200 dpi and checked image by image."),
        ("IMG_4822", "The HDR clip becomes an upright SDR MP4 that any portal can play."),
    ):
        summary = card.locator("details.output summary", has_text=name)
        await pointer.glide(summary, 1.0, dx=-200)
        await pointer.click()
        await asyncio.sleep(0.4)
        await page.evaluate(
            """(name) => [...document.querySelectorAll('article.job-card details.output')]
              .find(item => item.textContent.includes(name))
              .scrollIntoView({ behavior: 'smooth', block: 'center' })""",
            name,
        )
        await call("caption", caption)
        await asyncio.sleep(3.4)

    before, after = total, (await card.locator(".stats div").nth(1).locator("strong").inner_text())
    await call("caption", None)
    await call("card", f"""<div class="tag">FREE · MIT LICENSE · NO ACCOUNT · NO TELEMETRY</div>
      <h1>{before} → {after}, verified.</h1>
      <p>Photos, scanned PDFs and phone videos under one size limit, processed locally.</p>
      <div><code>{repo}</code></div>""")
    await asyncio.sleep(4.0)
    recorder.mark("end")
    await asyncio.sleep(0.4)


async def record(args: argparse.Namespace) -> None:
    work = Path(args.work)
    files = [str(Path(args.inputs) / name) for name in INPUTS]
    missing = [name for name in files if not Path(name).is_file()]
    if missing:
        raise SystemExit(f"Missing demo inputs: {missing}")
    output = Path(args.output)
    output.mkdir(parents=True, exist_ok=True)
    if any(output.iterdir()):
        raise SystemExit(f"{output} must be empty so the demo writes fresh results")

    env = dict(os.environ, WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=WEBVIEW_ARGS)
    app = subprocess.Popen([str(Path(args.app).resolve())], env=env)
    try:
        deadline = time.time() + 30
        while True:
            try:
                urllib.request.urlopen(f"http://127.0.0.1:{PORT}/json/version", timeout=1).read()
                break
            except OSError:
                if time.time() > deadline:
                    raise
                time.sleep(0.25)
        async with async_playwright() as playwright:
            browser = await playwright.chromium.connect_over_cdp(f"http://127.0.0.1:{PORT}")
            page = browser.contexts[0].pages[0]
            session = await page.context.new_cdp_session(page)
            await session.send("Page.setBypassCSP", {"enabled": True})
            await session.send("Emulation.setDeviceMetricsOverride", {
                "width": WIDTH, "height": HEIGHT, "deviceScaleFactor": 0, "mobile": False,
            })
            await page.reload()
            await page.get_by_role("button", name="Browse files").wait_for(timeout=30_000)
            await page.locator(".drop-note", has_text="MP4").wait_for(timeout=30_000)
            await page.evaluate(OVERLAY, {"files": files, "folder": str(output)})
            await page.evaluate("() => document.fonts.ready")

            async def call(name: str, value) -> None:
                await page.evaluate("([name, value]) => window.__demo[name](value)", [name, value])

            recorder = Recorder(work)
            await recorder.start(page)
            await asyncio.sleep(0.5)
            await demo(call, page, recorder, Pointer(page), args.repo, args.limit_mb)
            await recorder.stop(work)
            await browser.close()
    finally:
        app.terminate()
        try:
            app.wait(timeout=10)
        except subprocess.TimeoutExpired:
            app.kill()


def render(args: argparse.Namespace) -> None:
    work = Path(args.work)
    frames = json.loads((work / "frames.json").read_text())
    marks = json.loads((work / "marks.json").read_text())
    start, end = marks["start"], marks["end"]
    fast_start, fast_end = marks["fast_start"], marks["fast_end"]
    factor = max(1.0, (fast_end - fast_start) / args.fast_seconds)

    def remap(moment: float) -> float:
        moment = min(max(moment, start), end)
        if moment <= fast_start:
            return moment - start
        if moment <= fast_end:
            return fast_start - start + (moment - fast_start) / factor
        return fast_start - start + (fast_end - fast_start) / factor + moment - fast_end

    lines = []
    for index, frame in enumerate(frames):
        following = frames[index + 1]["t"] if index + 1 < len(frames) else end
        duration = remap(following) - remap(frame["t"])
        if duration > 0:
            lines += [f"file 'frames/{frame['file']}'", f"duration {duration:.5f}"]
    lines.append(lines[-2])
    (work / "frames.txt").write_text("\n".join(lines) + "\n")

    mp4, gif = Path(args.mp4 or work / "filecraft-demo.mp4"), Path(args.gif)
    mp4.parent.mkdir(parents=True, exist_ok=True)
    gif.parent.mkdir(parents=True, exist_ok=True)
    ffmpeg = ["ffmpeg", "-hide_banner", "-loglevel", "error", "-y"]
    subprocess.run(ffmpeg + [
        "-f", "concat", "-safe", "0", "-i", str(work / "frames.txt"),
        "-vf", "fps=30,scale=trunc(iw/2)*2:trunc(ih/2)*2:flags=lanczos,format=yuv420p",
        "-c:v", "libx264", "-preset", "slow", "-crf", "20", "-movflags", "+faststart", str(mp4),
    ], check=True)
    subprocess.run(ffmpeg + [
        "-i", str(mp4), "-vf",
        f"fps={args.gif_fps},scale={args.gif_width}:-1:flags=lanczos,split[a][b];"
        "[a]palettegen=max_colors=160:stats_mode=diff[p];"
        "[b][p]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle",
        str(gif),
    ], check=True)
    seconds = remap(end)
    print(json.dumps({
        "mp4": str(mp4), "gif": str(gif), "seconds": round(seconds, 2),
        "fast_forward": round(factor, 1), "job_seconds": round(fast_end - fast_start, 1),
        "mp4_bytes": mp4.stat().st_size, "gif_bytes": gif.stat().st_size,
    }, indent=2))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--app", help="Path to filecraft-desktop.exe")
    parser.add_argument("--inputs", help="Folder containing the files from make_inputs.py")
    parser.add_argument("--output", help="Empty folder for FileCraft results")
    parser.add_argument("--work", required=True, help="Scratch folder for frames and timing")
    parser.add_argument("--repo", default="github.com/canblmz1/filecraft")
    parser.add_argument("--limit-mb", default="5", help="Batch limit typed into the goal panel")
    parser.add_argument("--mp4", help="Rendered MP4 path (default: <work>/filecraft-demo.mp4)")
    parser.add_argument("--gif", default="docs/media/filecraft-demo.gif")
    parser.add_argument("--fast-seconds", type=float, default=4.5)
    parser.add_argument("--gif-width", type=int, default=880)
    parser.add_argument("--gif-fps", type=int, default=12)
    parser.add_argument("--render-only", action="store_true")
    args = parser.parse_args()
    if not args.render_only:
        if not (args.app and args.inputs and args.output):
            parser.error("--app, --inputs and --output are required unless --render-only is set")
        asyncio.run(record(args))
    render(args)


if __name__ == "__main__":
    main()
