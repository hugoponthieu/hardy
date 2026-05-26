# Decoding CSPCL `candump` Frames

This note explains how to read raw SocketCAN `candump` output for the
Hardy `cspcl` path that uses libcsp over CAN.

The goal is to answer three practical questions when looking at frames such as:

```text
vcan0  TX - -  0208100C   [8]  44 12 A7 02 00 1D 00 00
vcan0  TX - -  020C0C0C   [8]  00 04 00 00 27 10 00 00
vcan0  TX - -  020C080C   [8]  03 E8 00 00 00 01 00 00
vcan0  TX - -  020C040C   [8]  00 FA 00 00 00 02 08 1E
vcan0  TX - -  020C000C   [3]  0A 00 00
```

1. How do I decode the 29-bit CAN ID?
2. How do I tell whether CAN fragmentation, CSP SFP fragmentation, or RDP is in use?
3. Which frames are handshake/control traffic and which are data-bearing?

This document is based on the active libcsp source used by Hardy:

- `include/csp/interfaces/csp_if_can.h`
- `include/csp/csp_types.h`
- `src/interfaces/csp_if_can.c`
- `src/csp_sfp.c`
- `src/transport/csp_rdp.c`

## Three Different Things Called "Fragmentation"

The word "fragmentation" is overloaded here. Keep these three layers separate.

### 1. CAN-layer fragmentation: CFP

CAN carries at most 8 data bytes per frame. libcsp therefore uses the CAN
Fragmentation Protocol, or CFP, to split one logical CSP packet across many CAN
frames. This is encoded in the 29-bit CAN ID.

This is what the `Type`, `Remain`, and `Identifier` fields in the CAN ID are
for.

### 2. CSP SFP fragmentation: `CSP_FFRAG`

`csp_sfp_send()` splits a large blob into multiple CSP packets and sets the CSP
flag `CSP_FFRAG = 0x10`.

In libcsp 1.x, the SFP metadata is appended inside the CSP packet data area.
It is not a separate CAN-level header.

### 3. RDP transport: `CSP_FRDP`

`csp_connect(..., CSP_O_RDP)` enables the reliable datagram transport and sets
the CSP flag `CSP_FRDP = 0x02`.

The RDP header is also appended inside the CSP packet data area. It is not
visible as a separate outer header in the CAN frame.

### What This Means in Practice

- Many CAN frames with the same CFP identifier means one fragmented CSP packet
  at the CAN layer.
- CSP flags `0x12` means `CSP_FRDP | CSP_FFRAG`, so RDP and SFP are both active.
- CSP flags `0x02` means RDP is active without SFP.
- CSP flags `0x10` means SFP is active without RDP.
- None of this means the BP bundle itself is a BP fragment.

## Step 1: Decode the 29-bit CAN ID

libcsp defines the CAN ID layout in `csp_if_can.h`:

- Source: 5 bits
- Destination: 5 bits
- Type: 1 bit
- Remain: 8 bits
- Identifier: 10 bits

The fields are extracted as:

```text
src    = (id >> 24) & 0x1f
dst    = (id >> 19) & 0x1f
type   = (id >> 18) & 0x01
remain = (id >> 10) & 0xff
ident  =  id        & 0x3ff
```

`Type` is the key first discriminator:

- `0` means `CFP_BEGIN`: first CAN fragment of a CSP packet
- `1` means `CFP_MORE`: continuation CAN fragment

`Remain` counts down toward zero across the fragment train.

`Identifier` is the per-packet CFP ID. Frames with the same source,
destination, and identifier belong to the same logical CSP packet.

### Example: `0208100C`

For CAN ID `0x0208100C`:

- source = `2`
- destination = `1`
- type = `0` (`CFP_BEGIN`)
- remain = `4`
- identifier = `12`

So this is the first CAN fragment of a CSP packet from node 2 to node 1, and
there are four more CAN fragments after it with the same CFP identifier `12`.

That is exactly what appears next:

```text
0208100C  begin  remain=4  ident=12
020C0C0C  more   remain=3  ident=12
020C080C  more   remain=2  ident=12
020C040C  more   remain=1  ident=12
020C000C  more   remain=0  ident=12
```

This five-frame sequence is one logical CSP packet, fragmented only because the
underlying transport is CAN.

## Step 2: Understand What the `BEGIN` Frame Carries

libcsp's `csp_can_tx()` builds the first CAN fragment like this:

- 4 bytes: big-endian CSP header
- 2 bytes: big-endian CSP payload length
- 0 to 2 bytes: start of the CSP packet data

So a `BEGIN` frame does not start with RDP or SFP metadata. It starts with the
normal 32-bit CSP header and a 16-bit CSP packet length.

For this frame:

```text
0208100C   [8]  44 12 A7 02 00 1D 00 00
```

the fields are:

- CSP header = `44 12 A7 02`
- CSP packet data length = `0x001d` = `29`
- first 2 bytes of CSP packet data = `00 00`

The remaining four CAN frames with identifier `12` carry the other 27 bytes of
the same CSP packet data.

## Step 3: Decode the CSP Header

libcsp ships `utils/cspsplit.py`, which matches the bit layout used here:

```text
priority         = (hdr >> 30) & 0x03
source           = (hdr >> 25) & 0x1f
destination      = (hdr >> 20) & 0x1f
destination port = (hdr >> 14) & 0x3f
source port      = (hdr >>  8) & 0x3f
flags            =  hdr        & 0xff
```

The CSP flags relevant here come from `csp_types.h`:

- `CSP_FFRAG = 0x10`
- `CSP_FRDP = 0x02`
- `CSP_FXTEA = 0x04`
- `CSP_FCRC32 = 0x01`

### Example: `44 12 A7 02`

Header `0x4412A702` decodes as:

- priority = `1`
- source = `2`
- destination = `1`
- destination port = `10`
- source port = `39`
- flags = `0x02`

The key point is `flags = 0x02`, which means:

- RDP is active
- SFP is not active for this packet

So the `0208100C` sequence is an RDP packet carried over CAN CFP fragmentation,
but not an SFP data packet.

## Worked Example 1: RDP SYN Handshake Packet

This sequence:

```text
vcan0  TX - -  0208100C   [8]  44 12 A7 02 00 1D 00 00
vcan0  TX - -  020C0C0C   [8]  00 04 00 00 27 10 00 00
vcan0  TX - -  020C080C   [8]  03 E8 00 00 00 01 00 00
vcan0  TX - -  020C040C   [8]  00 FA 00 00 00 02 08 1E
vcan0  TX - -  020C000C   [3]  0A 00 00
```

is a good example of a pure RDP control packet.

Why:

- Same `(src=2, dst=1, ident=12)` across all frames, so this is one CSP packet.
- `BEGIN` frame says CSP packet data length is `29` bytes.
- CSP header flags are `0x02`, so RDP is active and SFP is not.
- In libcsp, `csp_rdp_send_syn()` builds a SYN packet with 24 bytes of RDP
  option data, then `csp_rdp_send_cmp()` appends the 5-byte RDP control header.
- `24 + 5 = 29`, which exactly matches this packet length.

That makes this sequence a strong match for an active-connect RDP SYN packet.

The payload bytes line up with the default RDP option block:

```text
00 00 00 04   window size
00 00 27 10   connection timeout = 10000 ms
00 00 03 E8   packet timeout = 1000 ms
00 00 00 01   delayed ACKs = 1
00 00 00 FA   ACK timeout = 250 ms
00 00 00 02   ACK delay count = 2
08 1E 0A 00 00   trailing 5-byte RDP control header
```

The last 5 bytes are the RDP control header appended by `csp_rdp_send_cmp()`.

## Worked Example 2: Short RDP Control Reply

These two packets are the short reply side of that handshake pattern:

```text
vcan0  TX - -  01100412   [8]  42 29 CA 02 00 05 0C 49
vcan0  TX - -  01140012   [3]  92 1E 0A

vcan0  TX - -  0208040D   [8]  44 12 A7 02 00 05 04 1E
vcan0  TX - -  020C000D   [3]  0B 49 92
```

For `01100412`:

- source = `1`
- destination = `2`
- type = `BEGIN`
- remain = `1`
- identifier = `18`

For the CSP header `42 29 CA 02`:

- source = `1`
- destination = `2`
- destination port = `39`
- source port = `10`
- flags = `0x02`
- CSP packet data length = `5`

The important signal is the length:

- `5` bytes is exactly the size of an RDP control header with no extra payload
- flags are `0x02`, so this is still RDP, not SFP

These are therefore small RDP control packets. On the wire they look different
from the 29-byte SYN because they carry only the 5-byte RDP control header.

You cannot infer the exact control bits from the first CAN frame alone, because
the RDP header sits inside the CSP packet data, not at the front of the CAN
frame. But the packet length and the surrounding sequence make it clear that
this is handshake/control traffic rather than bulk data.

## Worked Example 3: RDP + SFP Data Packet

This is the clearest example of both RDP and SFP being active:

```text
vcan0  TX - -  0208340E   [8]  84 12 A7 12 00 68 01 01
vcan0  TX - -  020C300E   [8]  00 00 00 00 00 00 00 00
vcan0  TX - -  020C2C0E   [8]  9F 89 07 00 02 82 02 82
vcan0  TX - -  020C280E   [8]  01 19 10 92 82 02 82 02
vcan0  TX - -  020C240E   [8]  19 10 92 82 02 82 02 19
vcan0  TX - -  020C200E   [8]  10 92 82 1B 00 00 00 C1
vcan0  TX - -  020C1C0E   [8]  6D D0 0E 17 1A 00 0F 0A
vcan0  TX - -  020C180E   [8]  20 19 EA 60 44 FE D5 43
vcan0  TX - -  020C140E   [8]  B5 86 06 02 02 02 45 82
vcan0  TX - -  020C100E   [8]  02 82 02 00 44 3C 57 83
vcan0  TX - -  020C0C0E   [8]  0D 86 01 01 04 02 44 74
vcan0  TX - -  020C080E   [8]  65 73 74 44 1B 27 E9 F7
vcan0  TX - -  020C040E   [8]  FF 00 00 00 00 00 00 00
vcan0  TX - -  020C000E   [6]  5B 04 1E 0B 49 92
```

The CAN ID `0208340E` decodes to:

- source = `2`
- destination = `1`
- type = `BEGIN`
- remain = `13`
- identifier = `14`

So this is one CSP packet fragmented across 14 CAN frames.

The CSP header `84 12 A7 12` decodes to:

- priority = `2`
- source = `2`
- destination = `1`
- destination port = `10`
- source port = `39`
- flags = `0x12`

`0x12 = 0x10 | 0x02`, so:

- `CSP_FFRAG` is set: SFP is active
- `CSP_FRDP` is set: RDP is active

This is the signature of a data-bearing transfer sent through
`csp_sfp_send()` on top of an RDP connection.

The `BEGIN` frame also gives the CSP packet data length:

- `00 68` = `104` bytes of CSP packet data

There are two useful observations here:

- The long CFP train tells you this is a large CSP packet at the CAN layer.
- The visible ASCII bytes `74 65 73 74` in `020C080E` spell `test`, which is a
  strong hint that this packet carries application data rather than only
  transport control metadata.

The exact byte layout inside those 104 bytes is:

- application data first
- then the 8-byte SFP trailer/header metadata appended by `csp_sfp_send()`
- then the 5-byte RDP header appended by `csp_rdp_send()`

That ordering matters. The first bytes after the `BEGIN` frame's 6-byte CFP
overhead are application bytes, not the RDP header.

## Worked Example 4: Handshake Followed by First Data Packet

This shorter sequence is useful because it shows the transition from pure RDP
control to RDP+SFP data:

```text
vcan0  TX - -  01101014   [8]  42 22 B4 02 00 1D 00 00
vcan0  TX - -  01140C14   [8]  00 04 00 00 27 10 00 00
vcan0  TX - -  01140814   [8]  03 E8 00 00 00 01 00 00
vcan0  TX - -  01140414   [8]  00 FA 00 00 00 02 08 0F
vcan0  TX - -  01140014   [3]  FC 00 00

vcan0  TX - -  02080410   [8]  44 1D 0A 02 00 05 0C 5F
vcan0  TX - -  020C0010   [3]  6B 0F FC

vcan0  TX - -  01100C16   [8]  82 22 B4 12 00 17 01 04
vcan0  TX - -  01140816   [8]  00 00 00 00 00 00 00 00
vcan0  TX - -  01140416   [8]  00 00 00 00 00 00 00 0A
vcan0  TX - -  01140016   [5]  04 0F FD 5F 6B
```

Interpretation:

- `01101014 ... 00 1D ...` is another 29-byte RDP SYN packet from node 1 to
  node 2. Flags are `0x02`, so this is RDP without SFP.
- `02080410 ... 00 05 ...` is the short 5-byte RDP control response from node 2.
- `01100C16 ... 00 17 ...` is the first small data packet after the handshake.
  Its CSP flags are `0x12`, so RDP and SFP are both active.

This is the pattern to look for in captures:

- `0x02` packets with length `29` often indicate the RDP SYN option block.
- `0x02` packets with length `5` are small RDP control packets.
- The first `0x12` packet after that is typically the beginning of actual
  application transfer over SFP-on-RDP.

## Quick Reference

### CFP CAN ID Fields

| Field | Bits | Meaning |
| --- | ---: | --- |
| Source | 5 | CSP source node address |
| Destination | 5 | CSP destination node address |
| Type | 1 | `0 = BEGIN`, `1 = MORE` |
| Remain | 8 | Remaining CAN fragments after this one |
| Identifier | 10 | CFP packet ID used to group fragments |

### How To Recognize Common Cases

| Signal | Meaning |
| --- | --- |
| Same `(src, dst, identifier)` across many frames | One logical CSP packet fragmented at the CAN layer |
| `Type = 0` | First CAN fragment of a CSP packet |
| `Type = 1` | Continuation CAN fragment |
| CSP flags `0x02` | RDP active, SFP not indicated |
| CSP flags `0x10` | SFP active, RDP not indicated |
| CSP flags `0x12` | RDP and SFP both active |
| CSP packet length `29` with flags `0x02` | Strong match for RDP SYN with 24-byte option block + 5-byte control header |
| CSP packet length `5` with flags `0x02` | Pure RDP control packet |

### Things That Are Easy To Get Wrong

- CAN fragmentation and SFP fragmentation are different mechanisms.
- `CSP_FFRAG` means SFP is in use. It does not mean the BP bundle is a BP fragment.
- The `BEGIN` CAN frame starts with the CSP header and CSP length, not with the
  RDP header.
- In libcsp 1.x, RDP and SFP metadata are appended inside the CSP packet data
  area, so exact control-bit interpretation requires reassembling the logical
  CSP packet, not inspecting a single CAN frame in isolation.
