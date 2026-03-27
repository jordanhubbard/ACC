# Overnight Render Queue

Drop `.blend` files into `input/`, they render overnight on Sparky's RTX GPU, results land in `output/`.

## Quick Start

```bash
# Drop your .blend file
cp my_scene.blend render_queue/input/

# Check queue status
node render_queue/render-queue.mjs --status

# Run manually (renders everything in input/)
node render_queue/render-queue.mjs

# Run with Slack notification when done
node render_queue/render-queue.mjs --notify

# Render only first N frames (e.g., preview)
node render_queue/render-queue.mjs --frames 10

# Set output format (PNG default, or JPEG/EXR/BMP)
node render_queue/render-queue.mjs --format JPEG
```

## Directories

| Path | Purpose |
|------|---------|
| `input/` | Drop `.blend` files here to queue them |
| `output/<name>/` | Rendered frames appear here |
| `done/` | Processed `.blend` files moved here |
| `render-log.json` | Full run history (last 100 jobs) |
| `render-notify.txt` | Slack notification queue (picked up by Natasha's heartbeat) |

## Overnight Cron

Renders run automatically at **11:00 PM PT** via cron job `overnight-render-queue`.

After rendering, Natasha sends you a Slack DM summary.

## Notes

- Blender 4.0.2 installed on Sparky (`/usr/bin/blender`)
- RTX GPU available for Cycles render engine
- Max 4h per file (prevents runaway renders)
- Failed files stay in `input/` for manual retry
- Multiple files render sequentially (not parallel — GPU is single-tenant)

## Render Settings

Your `.blend` file's render settings are used as-is. To use Cycles + GPU:

1. Open in Blender
2. Render Properties → Render Engine → Cycles
3. Cycles → Device → GPU Compute
4. Save and drop in queue

Natasha will not override your render settings — what you configure is what renders.
