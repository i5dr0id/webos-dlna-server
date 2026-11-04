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

## Rust version (recommended)

`rust/` contains a drop-in replacement written in Rust: a single ~450KB static binary using ~200KB of RAM, versus ~80MB for the Node.js server. Node.js 8 on 32-bit ARM can't open files larger than 2GB (nodejs/node#19455), so the Node server shells out to `dd`/`cat` for big files; the Rust server reads them natively.

It also works around a Chrome 68 bug on 32-bit webOS: `Content-Length` is truncated to 32 bits, so a range response of exactly N×4GB looks empty and the player reports "This file cannot be recognised". Open-ended range requests (`bytes=N-`, N > 0) are capped at 2GB chunks; the player requests the next chunk automatically.

Build (macOS, needs [rustup](https://rustup.rs) and the musl cross-linker):

```sh
rustup target add armv7-unknown-linux-musleabihf
brew install messense/macos-cross-toolchains/armv7-unknown-linux-musleabihf
cd rust && cargo build --release --target armv7-unknown-linux-musleabihf
scp target/armv7-unknown-linux-musleabihf/release/dlna-server root@<TV_IP>:/media/developer/dlna-server/dlna-server-rs
```

Init script (`/var/lib/webosbrew/init.d/dlna-server`):

```sh
#!/bin/sh
/media/developer/dlna-server/dlna-server-rs >> /tmp/dlna-server-rs.log 2>&1 &
```

The Rust version doesn't do ffprobe metadata yet.

### Playback notes

- **Dolby Vision Profile 7** (UHD Blu-ray REMUX "DV" releases) plays but jumping exits the player on this TV. Strip DV to HDR10 losslessly:
  `ffmpeg -i in.mkv -map 0 -c copy -write_crc32 0 -bsf:v "dovi_rpu=strip=1,filter_units=remove_types=62|63" out.mkv`
- Use `-write_crc32 0` whenever remuxing MKVs with ffmpeg for this TV. ffmpeg's default CRC-32 elements appear to stop the player from finding the seek index (still being verified).

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
