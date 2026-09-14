# Finding It On Your Network

Unplug the board, move it to a socket in another room, and it is still at:

**<http://esp32.local>**

No IP address to remember, no router admin, no serial cable. This page explains
why that works, because it looks like magic and is worth understanding.

## The problem it solves

When the board joins your WiFi, your router assigns it an address — something
like `192.168.1.157`. The board prints that to the serial console, which is
fine while a USB cable is attached and useless once it is in a wall socket
upstairs.

Worse, the address can change. Router reboots, the board is off for a few days,
another device takes the slot — and now the address you wrote down is wrong.

## Ordinary DNS: asking a server

When you type `google.com`, your computer asks a **DNS server**: "what is the
address for google.com?" A server somewhere knows, and answers.

That works because google.com is registered in a global system of servers. Your
ESP32 is not, and never will be. There is nobody to ask.

## mDNS: shouting instead

**mDNS** — multicast DNS — throws out the server entirely.

When you type `http://esp32.local`, your computer does **not** ask any
particular machine. It broadcasts to every device on your local network at
once:

> "Is anyone here called `esp32`?"

Every device on the network hears it. Almost all ignore it. The board
recognises its own name and answers directly:

> "That's me. I'm at 192.168.1.157."

Your computer caches the answer and connects. Total time: milliseconds.

```text
        ┌──────────┐
        │ your PC  │   "anyone called esp32?"
        └────┬─────┘
             │  broadcast to everyone
    ┌────────┼────────┬─────────┐
    ▼        ▼        ▼         ▼
 [phone]  [board]  [printer]  [TV]
    ✗        │        ✗         ✗
             │
             ▼
     "that's me, 192.168.1.157"
```

The `.local` ending is the signal. It is reserved to mean exactly this: do not
use normal DNS, shout on the local network instead. Your operating system sees
`.local` and switches methods automatically.

## Why this is convenient

**No configuration anywhere.** Nothing needs to know the name in advance. Not
your router, not a DNS server, not the other devices. The board simply answers
when called.

**The address can change freely.** DHCP might hand out a different IP after a
reboot — the name still finds it, because the board answers to its name
whatever address it happens to hold.

**It is already built in.** Windows 10 and 11, macOS, and iPhones all speak
mDNS out of the box. Apple calls it Bonjour; Linux calls it Avahi. It is the
same mechanism behind `printer.local` and AirPlay finding your speakers.

## Why it is LAN-only

Broadcasts do not cross the internet. A shout reaches your house and stops at
the router.

So `esp32.local` works from your sofa and not from a coffee shop. For a home
gadget that is usually what you want, and it is also a mild security property —
the board is not exposed to the internet by accident.

## The board's side

One line, set **before** connecting:

```python
wlan = network.WLAN(network.STA_IF)
wlan.active(True)
wlan.config(hostname="esp32")   # <- this is the whole trick
wlan.connect(SSID, PASSWORD)
```

!!! warning "Order matters"

    Setting the hostname *after* `connect()` is too late — the announcement
    has already gone out and the board will answer to its old name until it
    reconnects. Set it while the interface is up but not yet connected.

The name must be lower-case letters, digits and hyphens. No dots, no
underscores. You write `esp32` in code and type `esp32.local` in the browser —
the suffix is added by the naming system, not by you.

## Renaming it

`esp32` is generic, and two boards with the same name will collide. In
`main.py`:

```python
HOSTNAME = "garage"
```

Then redeploy, and it is at `http://garage.local`:

```powershell
.\deploy.ps1 -Port COM6
```

## A VPN will hide it

If one machine cannot reach the board while another can, **check for a VPN on
the machine that fails**. This is easy to misread as the board being down.

A VPN routes traffic through its tunnel, and unless told otherwise that
includes traffic meant for your own network. Requests to `192.168.1.157` go out
the tunnel instead of across the room, and never arrive. mDNS broadcasts get
swallowed the same way, so `esp32.local` stops resolving too.

The giveaway is that lower-level checks still succeed:

```powershell
arp -a | Select-String '80-65-99'
```

```text
192.168.1.157         80-65-99-f0-1c-9c     dynamic
```

The board is plainly there and answering at the link layer, while HTTP times
out. That combination means something is filtering, not that the board is gone.

Check what is running:

```powershell
Get-NetAdapter | Where-Object Status -eq 'Up' | Select-Object Name, InterfaceDescription
```

A `Mullvad Tunnel`, `Cisco AnyConnect`, `TAP-Windows`, or similar in that list
is the culprit. Either disconnect it, or turn on the client's **local network
sharing** option — most VPNs have one, and it keeps the tunnel up for internet
traffic while letting LAN addresses route normally.

!!! tip "Test from a second device first"

    A phone on the same WiFi, with no VPN, settles in seconds whether the
    problem is the board or the computer you are sitting at. Worth doing before
    investigating power supplies or reflashing anything.

## When mDNS does not work

Rare on a home network, but it happens:

- **Some Android versions** have patchy support
- **Corporate or guest networks** often block multicast entirely
- **VPN clients** can swallow local traffic

Two fallbacks.

### Find the IP in your router

Open your router's admin page (`http://192.168.1.1` for most Verizon boxes),
find the connected-devices list, and look for the board's MAC address —
`80:65:99:f0:1c:9c` for this one. The current IP is listed next to it.

The status page shows the MAC too, so you do not need a cable to look it up:

```text
Address: 192.168.1.157  (MAC 80:65:99:f0:1c:9c)
```

### Pin the address with a DHCP reservation

Better than looking it up repeatedly: tell the router to *always* give that MAC
the same address. In the router admin, look for "DHCP reservation" or "static
lease", and pair `80:65:99:f0:1c:9c` with `192.168.1.157`.

Five minutes, no code, and the address never moves again.

!!! tip "Do both"

    mDNS for convenience, a DHCP reservation underneath as the guarantee. Then
    `esp32.local` works day to day, and a fixed IP is there when it does not.
