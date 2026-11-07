# XEasyLabel HP4: reverse-engineered protocol

Source: `XEasyLabel.apk`, package `com.labelprinter.app` 3.28.1 (versionCode 32810),
sha256 `111c2b91…594c`. Analysed with [rea](https://github.com/morluto/rea) 6.8.0
(JADX headless bridge for DEX, Ghidra 12.1.4 for `lib/arm64-v8a/libykDataPacket.so`,
sha256 `71f260b9…eaea0`). Native config tables and reference outputs were taken by
running the library's own code under the Unicorn ARM emulator during the analysis
(the emulator harness isn't part of this repo).

## Transport

* Bluetooth Classic **SPP / RFCOMM**, UUID `00001101-0000-1000-8000-00805F9B34FB`,
  opened with `createInsecureRfcommSocketToServiceRecord` (no pairing PIN needed by the app).
  `com.labelprinter.printer.PrinterManager$c.run()`.
* Raw bytes are written to the socket's OutputStream; a reader thread feeds every
  `read()` into `YkDataPacket.unPkgMsg2()`.
* Right after connecting, the app sends *get serial* (+50 ms), then *get firmware* (+200 ms),
  then `YkDataPacket.init(0)` (`setCodeMethod(0)`, local only, nothing is sent).
* Advertised names that map to the HP4 profile: `DS-702`, `KD1`, `HP4`, `LP4`.

## Frame (both directions)

```
0x64 | cmd | seq | len u16 LE | payload[len] | 00 00 00 00 | 0x9B
```

`seq` is a global 6-bit counter that `cusPkgMsg()` increments before every frame.
The 4 bytes before `0x9B` stay zero on the HP4 print path. `cusSetChecksum` exists
(`0x12345678 + Σbytes + last`) but nothing on this path calls it.

## Host → printer

| cmd  | name              | payload                     |
|------|-------------------|-----------------------------|
| 0x00 | raster            | ≤ 24 rows × 12 bytes        |
| 0x02 | feed              | u16 dots                    |
| 0x03 | specFeedPaper     | u8 mode, u16 distance       |
| 0x04 | feed back         | u16 dots                    |
| 0x08 | auto power-off    | u8, ×10 min, 0 = never      |
| 0x09 | density           | u8 (0..15)                  |
| 0x0A | speed             | u8                          |
| 0x10 | get status        | none                        |
| 0x11 | get firmware      | none                        |
| 0x12 | get serial        | none                        |

Raster rows: 96 dots (12 mm head at 8 dots/mm = 203 dpi), MSB = leftmost dot, 1 = black.

## Print job (`pkgPrintLabelWithConfig`, driven by a config table)

Config tables are `vector<pair<string,int>>` built at static-init time and read from the
emulated library:

```
configHP4     : pointsPerMM 8, dotsNum 96, pkgLength 24, protoVer 1 (AstraP1), alignment 0,
                setSpeed 50, setDensity 12, first&feedback 24, ImgWithRightMargin -4,
                feed 40, last&feed 24
configHP4_GAP : pointsPerMM 8, dotsNum 96, pkgLength 24, protoVer 1, alignment 0,
                miliDisRatio 1016, setSpeed 50, setDensity 12, first&specFeedPaper 2,
                feedToMid 0, ImgWithRightMargin -4, last&specFeedPaper 1, nonLast&specFeedPaper 0
```

`setSpeed`/`setDensity` are replaced by the caller's values. Java passes speed 25
(`PrinterType` HP4) and a density from `DENSITY_5_9_13` (light 5 / normal 9 / dark 13).

Continuous tape (`HP4`):
`speed, density, [first: feedBack 24], raster…, feed 40, [last: feed 24]`

Die-cut labels (`HP4_GAP`):
`speed, density, [first: specFeed(2, 800)], [feed v], raster…, specFeed(last ? 1 : 0, 800)`
with `v = trunc(8·(labelLength − printX) − 1.008·L)` (L = image length in dots);
a negative `v` drops `|v|` rows from the start of the image.

## Image pipeline (Java `PrinterBase.f0/j0` + native)

1. The label bitmap (text running along the tape) is scaled to `printW × 8` dots.
2. Native rotates by `rotateDegreeType` (all HP4 media: `rotate = 3` → 270), which is
   PIL `ROTATE_90` (counter-clockwise).
3. The band is centred on the 96-dot head, then shifted 4 dots right (`ImgWithRightMargin -4`).
4. Threshold: black iff `(R+G+B)//3 <= 128`; alpha is ignored.

## Printer → host status (`cmd 0x80/0x81`, `cusUnpkgPrinterStatus`)

Payload: `i16 voltageRaw, u16 flags, u8 density, u8 autoOff (×10 min), u8 speed, [u8 battery%]`.
The HP4 sends these unprompted as `cmd 0xFF` about every 5 s while connected. Unlike the host,
it fills the checksum field in its own frames.
Voltage in mV = `raw · 3340 · 280 / 409600`. With no battery byte, the percentage comes
from a voltage table (one table for charging, one for not charging).

| bit | flag          | bit | flag           |
|-----|---------------|-----|----------------|
| 0   | on            | 8   | cover open     |
| 1   | bt connected  | 9   | low battery    |
| 2   | charging      | 10  | overheating    |
| 3   | printing      | 11  | paper out      |
| 4   | suspended     | 12  | print allowed  |
| 5   | usb busy      | 13  | valid paper    |
| 6   | bt busy       | 14  | upgrading      |
| 7   | motor out sync| 15  | paper loaded   |

Other reply types (`cusUnpkgReceivedMsgDataType`): `0xF1` firmware info, `0xFF`/`0x81`
status notifications, `0x73` paper type, `0xFE` upgrade state.

## Auto power-off

`PrinterBase.b(int)` → `YkDataPacket.getSetAutoOffTimeCommand` → `cusSetAutoOffTime` builds
`cmd 0x08` with one byte in units of 10 minutes. The app (`PrinterBase.c()`) offers
0 = never, 1 = 10 min, 3 = 30 min, 6 = 1 h, 18 = 3 h, 30 = 5 h, 60 = 10 h.
Tested on a real HP4 (firmware 0x2016): the new value shows up in the next status
report. The printer sometimes drops the link right after the change.
