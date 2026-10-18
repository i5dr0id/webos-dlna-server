# webOS DLNA Media Server

A lightweight DLNA/UPnP media server for rooted LG webOS TVs. Serves media files from a USB drive to the native Photo & Video app over the local network — no external dependencies, runs on Node.js 8+.

## Why

ExFAT-formatted USB drives mounted via FUSE on rooted webOS TVs aren't recognized by the native Photo & Video app (the Physical Device Manager reports `UNSUPPORT_FILESYSTEM`). This server bridges the gap: it runs on the TV itself, advertises as a DLNA media server, and serves files from the USB mount so the Photo & Video app can browse and play them.

## Features

- **Zero dependencies** — uses only Node.js built-in modules
- **SSDP discovery** — auto-discovered by the TV's Photo & Video app
- **Folder browsing** — full ContentDirectory service with nested folder navigation
- **Media metadata** — video duration, resolution, and bitrate via ffmpeg/ffprobe
- **Subtitle support** — detects `.srt`, `.sub`, `.ass`, `.ssa`, `.vtt` files matching video names and serves them via `sec:CaptionInfoEx`
- **HTTP range requests** — seeking and fast-forward support
- **DLNA protocol info** — proper `DLNA.ORG_OP` and `DLNA.ORG_FLAGS` for player compatibility
- **Auto USB detection** — scans `/tmp/usb` for mounted drives, handles drive letter changes on reconnect

## Requirements

- Rooted LG webOS TV (tested on webOS 5.6.0, 55UN6800PVA)
- Node.js 8+ (pre-installed on webOS TVs)
- USB drive mounted (e.g. via [exfat-automount](https://github.com/nicedoc/exfat-automount) for ExFAT drives)
- Optional: ffmpeg static ARM binary for media metadata (duration, resolution)

## Install

```sh
# Copy server to TV
scp server.js root@<TV_IP>:/media/developer/dlna-server/

# Optional: copy ffmpeg static binary for metadata support
# Download ARM static build from https://johnvansickle.com/ffmpeg/
scp ffmpeg-linux-arm root@<TV_IP>:/media/developer/dlna-server/ffprobe
ssh root@<TV_IP> 'chmod +x /media/developer/dlna-server/ffprobe'

# Start the server
ssh root@<TV_IP> 'node /media/developer/dlna-server/server.js &'
```

## Auto-start on boot

Create an init script at `/var/lib/webosbrew/init.d/dlna-server`:

```sh
#!/bin/sh
node /media/developer/dlna-server/server.js >> /tmp/dlna-server.log 2>&1 &
```

```sh
chmod +x /var/lib/webosbrew/init.d/dlna-server
```

## Usage

1. Start the server on the TV
2. Open the **Photo & Video** app from the home screen
3. A network device called **"USB Media Server"** appears
4. Browse folders and play media

## Configuration

Edit the top of `server.js`:

| Variable | Default | Description |
|----------|---------|-------------|
| `PORT` | `9000` | HTTP server port |
| `USB_ROOT` | `/tmp/usb` | Base path to scan for USB drives |
| `SERVER_NAME` | `USB Media Server` | Name shown in Photo & Video |
| `FFPROBE` | `/media/developer/dlna-server/ffprobe` | Path to ffmpeg/ffprobe binary |

## How it works

```
Photo & Video app
       |
       | SSDP discovery (239.255.255.250:1900)
       v
  DLNA Server (port 9000)
       |
       | UPnP ContentDirectory Browse (SOAP/XML)
       | HTTP file streaming (with range support)
       v
  USB mount (/tmp/usb/sdX/sdX1)
```

The server implements:
- **SSDP** — UDP multicast for device discovery
- **UPnP Device Description** — XML device and service descriptors
- **ContentDirectory:1** — SOAP service for browsing folders and listing media
- **ConnectionManager:1** — SOAP service reporting supported media types
- **HTTP streaming** — file serving with `Accept-Ranges` and DLNA transfer headers

## Supported formats

Video: mp4, mkv, avi, mov, webm, ts, flv, wmv, m4v, 3gp
Audio: mp3, flac, aac, ogg, wav, wma, m4a
Image: jpg, jpeg, png, gif, bmp, webp
Subtitles: srt, sub, ass, ssa, vtt

## License

MIT
