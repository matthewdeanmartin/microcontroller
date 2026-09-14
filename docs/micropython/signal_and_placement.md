# Signal and Placement

Where you put the board matters more than anything in the code. This page is
the measurement that proved it.

## Reading the number

The status page shows the board's view of the WiFi signal:

```text
Signal: -72 dBm (marginal)
```

RSSI is measured in **dBm**. It is always negative, and **closer to zero is
stronger**.

| Range | Quality | In practice |
|---|---|---|
| -30 to -60 | excellent | rock solid |
| -60 to -70 | good | reliable |
| -70 to -80 | **marginal** | mostly works, occasional timeouts |
| below -80 | unreliable | frequent failures |

It is a **logarithmic** scale, which is the part that surprises people. Every
**3 dB is a doubling or halving** of power:

- -60 → -70 dBm is **10× less** signal
- -60 → -72 dBm is about **16× less**

So the gap between "good" and "marginal" is far larger than the numbers look.

## A real measurement

The same board, same firmware, three positions in one house:

| Position | Signal | Behaviour |
|---|---|---|
| Next to the router | **-11 dBm** | perfect |
| Same floor as the router, across the room | **-60 dBm** | reliable |
| Four floors up | **-72 dBm** | **intermittent** |

Between the first and last is **61 dB** — a factor of over a million in
received power. Floors, walls and distance are that expensive.

At -72 dBm the board worked, but page loads occasionally timed out. Nothing was
wrong with the firmware; it simply had no margin left.

!!! note "-11 dBm is almost too strong"

    Below roughly -20 dBm some radios distort from overload, an effect called
    receiver desensitisation. It is not a problem in practice - the signal
    drops off quickly as soon as you move away - but it is why "as close as
    possible" is not automatically the goal.

## Downlink only

The number is how well **the board hears the router**. It says nothing about
whether the board's replies get back.

That matters, because the two directions are not symmetric:

- The router has multiple large antennas and plenty of power
- The board has a tiny chip antenna and runs off USB

So the **uplink is the weaker path**, and it can be marginal while the downlink
number still looks acceptable. If a board reports a decent RSSI but the
connection is flaky anyway, an asymmetric link is a likely explanation.

Your router's admin page usually shows the signal *it* receives from each
client. Comparing that with the board's own reading is the closest you can get
to seeing both directions.

## Finding a better spot

Signal is not just about distance. A few feet can be worth 10 dB.

- **Get it off the floor.** Floors and the ground absorb badly.
- **Away from metal.** Appliances, filing cabinets, radiators, mirrors
  (which are metal-backed) all block or reflect.
- **Out of enclosures.** A metal box is a Faraday cage; even a thick plastic
  one costs a little.
- **Use open paths.** Signal travels far better up a stairwell or through a
  floor vent than through several concrete floors.
- **Avoid microwave ovens.** They operate at 2.4GHz and will stamp on the band
  while running.

The status page shows the signal live, so you can carry the board around and
refresh after each move. Aim for better than **-65 dBm**.

## Channel congestion

Signal strength is only half the story. The scan below shows the 2.4GHz band in
a dense neighbourhood:

```powershell
netsh wlan show networks mode=bssid |
    Select-String -Pattern 'SSID|BSSID|Band|Channel|Utilization'
```

```text
BSSID 2 : 3c:bd:c5:1a:7e:72
Band    : 2.4 GHz
Channel : 6
Channel Utilization: 188 (73 %)
```

**73% utilisation** means the channel is busy three-quarters of the time.
Transmissions collide, get retried, and sometimes fail outright.

A weak signal and a congested channel compound each other: retransmissions cost
more when each attempt is already marginal.

### Picking a channel

2.4GHz channels overlap heavily. **1, 6 and 11 are the only three that do not
overlap each other**, which is why nearly everyone uses them — and why a
neighbour on channel 7 interferes with your channel 6.

To change it on a Verizon Fios router:

1. Browse to `http://192.168.1.1`
2. Log in — the admin password is usually on a sticker on the router, and is
   **not** the WiFi password
3. Find **Wi-Fi Settings** → **Advanced** → the **2.4 GHz** tab
4. Change **Channel** from Auto (or 6) to **1** or **11**
5. Save

!!! warning "Change only the 2.4GHz radio"

    Leave 5GHz alone — everything else in the house is probably using it, and
    the ESP32-S2 cannot. Changing 2.4GHz briefly drops smart plugs, printers
    and thermostats while they reconnect.

Pick whichever of 1 or 11 has fewer neighbours in your scan, counting anything
within two channels as interference.

## Which problem do you have?

| Signal | Channel busy | Likely fix |
|---|---|---|
| better than -65 | low | not a radio problem; look elsewhere |
| better than -65 | high | change channel |
| -65 to -75 | low | reposition the board |
| -65 to -75 | high | reposition first, then change channel |
| worse than -75 | any | an extender or a second AP |

No firmware change affects any of this. It is physics, and the fix is always
placement, channel, or another access point.

## Measure, do not guess

Intermittent faults defeat one-off testing — every attempt eventually succeeds,
so any change appears to have helped.

```powershell
.\watch.ps1 -LogFile watch.csv
```

Polls `/health` on an interval and logs each result. `Ctrl+C` prints a summary:

```text
--- summary ---
  attempts : 180
  ok       : 172 (95.6%)
  failed   : 8
  rssi     : avg -71.4 dBm, range -76..-68
```

Run it for ten minutes before a change and ten minutes after. A **failure
rate** tells you whether a fix worked; a single successful page load does not.

`/health` includes the signal for exactly this reason:

```text
ok rssi=-72
```

so each logged sample carries the link quality with it, and you can see whether
failures cluster with dips.
