Prysm LED system — revision C hobby prototype, 2026-10-09

Raspberry Pi 5 on its own USB-C supply; BTF SK9822 5V, 60 LEDs/m,
12mm black PCB, IP30, about 2.8m / 168 LEDs installed. The selected strip
variant is 45759013650658 (5m reel); do not power the full reel as this load.
LED supply: Mean Well LPV-100-5, 5V 12A / 60W — not 15A and not 100W.
User-selected limit: 75% LINEAR final LED output, applied in software.
This package documents that cap; it does not implement or enforce it.
No mains wiring is on these PCBs. This is a personal, supervised prototype.

REVISION C CHANGES AND STATUS
Three PCBs: GPIO interface 65x30mm; DC distribution 75x80mm; bottom-right
corner interface 65x40mm. ISO7720FD isolated level translators replace the
AHCT125 and the passive bottom-right L. Both supplies and both grounds at
that corner remain separate on the corner board. The strip groups' grounds
meet only at the distribution board through their heavy return cables.
The GPIO board separates Pi ground from the LED ground, including grounds
reached through Pi USB/HDMI. The separate distribution-to-GPIO logic cable
and F4/J4 were removed. Each translator side gets power and ground from
its local source/receiver, avoiding an independently fused logic supply that
could fail while leaving driven inputs on an unpowered IC.

The two BTF T injection corners are retained as requested. Their internal
mapping, contact current capability and pad fit are NOT verified. BTF's
inspected listing supplies no current rating. These are commissioning
blockers, not a declaration that the connectors are unsuitable or safe.
The CAD/package checks cover actual nets, footprints, routing and exports;
physical temperature, voltage, fault response, pad fit and cooler clearance
still require the tests below. This revision has not been physically tested.

SIGNAL AND POWER TOPOLOGY (viewed from behind the TV)
One directed chain, following the actual strip arrows:
  Pi -> GPIO J3 -> TOP -> T at top-right -> RIGHT
     -> corner PCB J1 (A input) -> J2 (B output) -> BOTTOM
     -> T at bottom-left -> LEFT -> unused final DO/CO
Leave the top-left start/end gap OPEN. Insulate the LEFT output pads.
Never join LEFT DO/CO back to TOP DI/CI. Do not fit a parallel passive L at
bottom-right: it would bypass the isolator and join the power/ground groups.
The user-approved active corner replaces that L; no L connector is now used.
If all four geometric corners must later connect, move the signal gap to a
straight section and redesign power grouping/feeds before connecting it.

  Corner       Connection                           Power arm
  Top-right    TOP DO/CO -> RIGHT DI/CI, group A     +5V_A and GND only
  Bottom-left  BOTTOM DO/CO -> LEFT DI/CI, group B   +5V_B and GND only
  Bottom-right Active A-to-B corner PCB; no bypass   None
  Top-left     Open end/start gap                    None

Distribution J2 -> top-right T: A = TOP + RIGHT.
Distribution J3 -> bottom-left T: B = BOTTOM + LEFT.
A and B positives must not join downstream of F2/F3. No A-to-B GND bridge
is permitted in the strip/corner harness either: it would give LED current
an alternative return through strip copper if a heavy return disconnected.
The corner isolator transfers signals without a conducting data/clock path
between groups. Opening a branch fuse removes that group's LED and local
translator power together. Missing local return cannot make a DC LED return
through Pi, the opposite strip, or data/clock pins across an isolator.
This is low-voltage return-path separation, NOT a mains isolation design.
Keep strip copper, mounting hardware and module grounds off TV metalwork.

T CONNECTORS AND REMOVABLE POWER ARMS
Select T shape / 4pin / 12mm explicitly. The older linked variant
45648064020706 selected L / 4pin / 12mm / 10sets when inspected; it is NOT
an ordering link for the intended T. X may be used only after the same
mapping checks; its fourth arm stays unused and insulated.
The strip is IP30 (no waterproof sleeve). Width alone does not establish
pad spacing, insertion depth, contact pressure, or polarity compatibility.
Ignore analog RGB markings. With the actual strip facing the intended way,
identify every connector arm/contact by continuity to actual strip labels:
  upstream DO -> downstream DI; upstream CO -> downstream CI;
  +5 -> +5; GND -> GND; no cross-connections and no joined outputs.
Record the complete mapping before power. There must be exactly one
upstream strip output and one downstream strip input per T. Never attach
an LED section to the power arm, even if its power wiring is correct.

A clip arm accepts a strip tongue, not loose wires. Use a sacrificial piece
of this exact 12mm strip PCB as a blank power tongue: remove all LEDs/ICs
and other components; cut/isolate the DATA/CLOCK tracks beyond their contact
pads; verify there is no connected active circuitry and no path from those
pads to either power lead. Solder short 16AWG +5/GND leads to the identified
power copper outside the clamp area. Keep solder and insulation out of the
contacts and preserve the original contact surface. If the remaining power
copper or pad is too small/fragile for a sound joint, this adapter has NOT
passed: obtain a purpose-made matching blank tongue before commissioning.
Do not invent a pad footprint from the advertised width.

Each power tongue has an XT30U-M pigtail; the energized branch cable has
XT30U-F recessed contacts. Two matched inline pairs total. The T's power
arm DATA/CLOCK pads have no attached conductors or LEDs; internal unloaded
stubs on the T do not create a second LED chain. Sleeve the soldered tongue
and capacitor leads, anchor the cable independently, and leave the clip
accessible. The XT30 disconnect lets the T/tongue remain fitted during
removal. Both supplies OFF before any connector is unplugged.
Fit one 1000uF 10V capacitor across each power tongue's +5/GND, insulated.
Do not treat the capacitor or brightness setting as connector protection.

LOAD, SUPPLY AND FUSES
168 x 0.06A = 10.08A nominal uncapped white, about 5.04A per equal group.
At 75% linear output the estimate is 7.56A total / 3.78A per equal group,
plus idle/logic current and real strip variation. This estimates average
load, not guaranteed peak/RMS current: LED PWM can retain full pulse
amplitude, and contact heating must be measured. Keep an upper design count
of 90 LEDs/group, 180 total (5.4A/group, 10.8A uncapped). Count actual cuts.
Size harnesses for uncapped operation; software is not fault protection.
Apply the cap at final pixel output, including startup and test effects.

LPV-100-5 delivers 12A through 40C ambient at rated input; its 5V-model curve
derates above 40C to 60% at 70C. At 50C the curve is about 10.4A available.
Allow ventilation and measure temperature behind the TV. Output tolerance
is +/-8% including line/load: 4.6..5.4V before wiring losses. Verify loaded
voltage at all strip ends; do not raise voltage to hide an excessive drop.
Its overload mode is automatic-retry hiccup at 110..150% of rated power.
Fuses may not clear under a current-limited/hiccup supply. F1 and the inline
15A fuse are NOT 12A overload limiters or selective protection. The 297
7.5A fuse may take up to 600s at 135% and up to 5s at 200%, under datasheet
conditions. Continuous fuse capability also falls with ambient temperature.
No claim is made that these fuses protect every PCB neck, strip IC, or
resistive connector fault. Poor contacts can overheat below fuse rating.

F1=15A; F2/F3=7.5A, Littelfuse 297 MINI blade, Keystone 3568 holders.
An additional inline 15A MINI fuse stays near PSU positive for input cable
protection, in a Littelfuse 0FHM0001SXJ covered MINI holder (20A/58VDC,
14AWG GXL leads). Its wires are black: mark both POSITIVE ends with red
sleeves before wiring. It has no guaranteed discrimination from F1. The old 2A logic
fuse and logic power cable are omitted; isolated logic supplies are local.
Insulated, anchored short wiring plus the LPV protection are part of this
prototype, with fault-current/temperature qualification still required.

CONNECTORS, WIRES AND LOCAL LOGIC POWER
Distribution J1: AMASS XT30UPB-M, pin1=GND, pin2=+5V_IN. Mate XT30U-F;
check molded polarity. PSU output is red+, black- (factory14AWG leads).
Use 14AWG copper total main pair target <=0.5m, including factory leads.
Distribution J2/J3: JST VH2 pin1=+5V_A/B, pin2=GND. VHR-2N mates,
SVH-41T-P1.1 contacts; 16AWG copper pair, target <=1m per branch.
JST specifies 10A with 16AWG under its test conditions; that is not a
rating for the BTF clips or the whole assembled harness.

GPIO J3 and corner J1/J2: JST VH4, same assigned contact order:
  1 = LOCAL strip +5V; 2 = DATA; 3 = CLOCK; 4 = LOCAL strip GND.
VHR-4N with SVH-41T-P1.1 contacts:16AWG power/ground,20AWG DATA/CLOCK.
Use copper wire with insulation rated >=105C and supplier current ratings
suited to the upstream fuse. No copper-clad aluminium or Dupont contacts.
Solder each four-wire pigtail to the identified strip pads, anchor it, and
insulate each conductor. Removability is at the VH connector, not by pulling
soldered strip pads. Keep GPIO-to-TOP <=30cm and each corner pigtail <=10cm;
route DATA/CLOCK close to its local GND. Solder tails must not touch TV metal.

GPIO J3 connects ONLY to the TOP INPUT end's local four pads. Its +5/GND
power only U1's output side. There is NO distribution-to-GPIO connection.
Corner J1 connects ONLY to RIGHT OUTPUT pads (local A rail and ground).
Corner J2 connects ONLY to BOTTOM INPUT pads (local B rail and ground).
These pigtails power only the small local isolator sections, never an LED
feed. Do not add cross-ground straps, shields bonded at both domains, or
any passive strip bridge around the corner module.

ISO7720FD (D8 narrow SOIC):1=VCC1,2=INA,3=INB,4=GND1,5=GND2,
6=OUTB,7=OUTA,8=VCC2. Use the default-LOW F variant and two forward channels;
ISO7721 or default-high ISO7720 are not substitutions. TI specifies
2.25..5.5V supply/level translation, default low with input-power loss,
and high impedance with output-power loss. Inputs require their own local
power; do not introduce separately switched/fused isolator-only rails.
Pi GPIO10/physical19=MOSI; GPIO11/physical23=SCLK; physical6=Pi GND;
physical1 supplies only U1's Pi side at3.3V. Pi5V pins2/4 remain NC.
10k input pulldowns and33ohm output resistors are fitted on both interfaces.
Each IC side has its own adjacent100nF bypass and local copper planes.
Start SPI mode0 at1MHz; framing and colour order must match this strip.
No chip select is required. Default-low behavior does not guarantee absence
of every power-ramp glitch; send an all-black frame before enabling effects.

ASSEMBLY AND FABRICATION
Open the three .kicad_pro files in KiCad10. Embedded symbols, local symbol
libraries and standard embedded footprints are included. Order2 layers,
FR4,1.6mm, mask both sides, lead-free HASL or ENIG. Distribution2oz BOTH
sides; GPIO/corner1oz both. Gerbers and separate PTH/NPTH Excellon files
are in each fabrication directory and its matching fabrication.zip.
Use prysm-led-system-rev-c.zip; revision B is superseded.

Only U1 on each interface is surface mount (SOIC8,1.27mm lead pitch).
Hand solder with flux and inspect all pins under magnification. Follow pin1
marking; DO NOT install the old PDIP14 AHCT125. Other parts are through-hole.
GPIO J1 is the ONLY underside component. Its pin1/square pad matches the
KiCad Pi uHAT convention; install Samtec ESQ-120-23-G-D below the PCB.
The socket body is16.13mm; actual mated spacing and4.83mm tail protrusion
must be checked with the Pi5 cooler/fan plug. GPIO holes are58mm apart.
Use M2.5 insulating spacers without forcing socket engagement or bending.
Corner and distribution boards use four insulated M2.5 mounting points.
Keep board undersides covered/clear of TV metal; retain airflow and access.
The generic socket3D model does not validate the selected tall socket fit.

Distribution C1 and both corner power capacitors: Panasonic EEU-FR1A102,
1000uF10V, D10xH16mm,5mm lead pitch. Square PCB pad is+. Negative stripe
faces GND. Fully solder all high-current connector/holder pins and inspect.
Crimp with the exact-terminal tool, perform pull tests, heat-shrink each
conductor, and provide anchors so no cable load reaches strip copper.

COMMISSIONING — RECORD ACTUAL VALUES BEFORE TV INSTALLATION
1. Supplies disconnected: inspect soldering, polarity, U1 pin1 and caps.
   Prove each T's four-contact mapping in its installed orientation, the
   power tongue's two-wire-only connection, and the open top-left signal gap.
   For A/B POSITIVE isolation remove F2 AND F3: upstream bus otherwise joins
   their fuse inputs. To verify no harness GND bridge, also unplug both VH2
   branches from distribution. The standalone corner must not have a DC
   A-to-B power/ground connection. Verify Pi GND-to-LED GND isolation too.
2. Use a current-limited bench5V supply initially, if available. Verify each
   branch and translator domain before attaching Pi. Establish Pi3.3V side
   and both local strip supplies; confirm no header5V back-power path.
3. Test a few LEDs dim red/green/blue, then all168. Verify colour order and
   one linear chain, with no reflected/parallel strip branch. Apply75% cap.
4. Measure main and branch currents at capped all-white and actual effects.
   Measure supply, both injections and all remote strip end voltages. Scope
   DATA/CLOCK at first TOP and first BOTTOM inputs for clean thresholds and
   overshoot relative to LOCAL supply/ground. Confirm supply never exceeds
   5.5V at isolators; obtain the actual SK9822 input/output limits if available.
5. Run to thermal equilibrium at the intended maximum ambient with the75%
   cap. Log T input and each strip-arm contact, power tongue, XT30, VH, fuse
   holders, wires, copper and supply temperatures. Check voltage across each
   connector to find contact resistance. No rating exists for BTF in the
   inspected listing: obtain manufacturer data or conservatively qualify the
   exact assembled sample, including repeated insertions and strain relief.
   An average3.78A estimate alone does not establish a safe T current limit.
6. On a current-limited bench, test Pi-only, LEDs-only, F2-open, F3-open and
   each branch-return-open case. Check isolated sides do not get parasitic
   power and no opposite branch/Pi wire becomes an LED return. Power OFF
   before reconfiguring each test; no live unplugging or intentional shorts
   against the full LPV supply. Fault-current/fuse-clearing tests need a
   controlled load, appropriate instrumentation and a nonflammable bench.
7. Dry-fit the real cooler/socket, pigtails and clips; test removal/reassembly
   without stressing strip pads. Mount behind TV only after those checks.

VALIDATION AND REBUILD
Use KiCad10.0.7's bundled Python (pcbnew), e.g. on this Mac:
  /Applications/KiCad/KiCad.app/Contents/Frameworks/Python.framework/Versions/Current/bin/python3 build.py
build.py is the CAD source of truth and OVERWRITES its generated CAD, reports,
previews, fabrication files and ZIPs. Edit the generator before rebuilding;
manual CAD edits must be transferred back to it. Routing tie-breaks and board
UUIDs are deterministic. All three boards are regenerated, then fresh ERC,
DRC with zone refill/parity and actual schematic/PCB pin-map checks run.
The script exports and packages only after checks pass. KiCad default ignored
rule classes still exist; no project-specific exclusions were added.

Run the same bundled Python with verify.py for fresh checks in temporary
storage, actual CAD net/footprint inspection, and SHA256/ZIP consistency.
It does not trust saved reports alone. build-manifest.json binds exported
files and reports to their generator/CAD/docs; changing them requires rebuild.
Exact CAD regeneration has been compared independently. KiCad reports do not
prove thermal performance, fault protection or the external harness mapping.

MANUFACTURER REFERENCES (reviewed2026-10-09)
Strip and selected variant:
https://www.btf-lighting.com/en-intl/products/1-sk9822-led-pixel-strip-data-and-clock-dc5v?variant=45759013650658
T/L/X connector family (select T4pin12mm manually; no published current rating):
https://www.btf-lighting.com/en-intl/products/2pin-3pin-4pin-5pin-corner-connector-8mm-10mm-12mm-width-t-l-x-shape-solderless-connector-ws2811-ws2812b-led-strip-no-soldering
Mean Well LPV-100:5V model table, overload, voltage tolerance,5V derating curve:
https://www.meanwell.com/Upload/PDF/LPV-100/LPV-100-SPEC.PDF
ISO7720/ISO7720F: D8 pinout§5, supply limits§6.3, power-loss modes§8.4,
decoupling§10. No mains safety claim is made for these boards:
https://www.ti.com/lit/ds/symlink/iso7720.pdf
JST VH: B2P/B4P-VH, VHR-2N/4N, SVH-41T-P1.1 (20–16AWG),10A at16AWG:
https://www.jst-mfg.com/product/pdf/eng/eVH.pdf
AMASS XT30UPB-M (manufacturer20A test rating with16AWG; mating must match):
https://www.china-amass.net/xt30upb-m-product/
Keystone3568 MINI fuse holder:
https://www.keyelco.com/product.cfm/product_id/306
Littelfuse 0FHM0001SXJ inline holder (20A/58VDC, 14AWG):
https://www.littelfuse.com/assetdocs/littelfuse-fuse-holder-mini-fhm-datasheet.pdf?assetguid=339fa35e-eabc-45c5-9093-8abceaa8a2cf
Littelfuse297 MINI ratings and time/current/temperature curves:
https://www.littelfuse.com/assetdocs/littelfuse-datasheet-297-mini32v?assetguid=42c9dd21-a88e-4328-8e67-2f832444faf1
Samtec ESQ-120-23-G-D:
https://www.samtec.com/products/esq-120-23-g-d
Panasonic EEU-FR1A102:
https://industrial.panasonic.com/ww/products/pt/aluminum-cap-lead/models/EEUFR1A102
Pi SPI0:
https://www.raspberrypi.com/documentation/computers/raspberry-pi.html#spi0
Pi5 cooler drawing:
https://datasheets.raspberrypi.com/cooling/raspberry-pi-active-cooler-mechanical-drawing.pdf
