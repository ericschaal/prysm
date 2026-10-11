SUPERSEDED: see ../led-system/README.txt and revision B KiCad projects.
This directory preserves the original concept only.

Prysm GPIO LED board — revision A concept

STATUS: review draft, not a fabrication package.

Purpose
Plug onto a Raspberry Pi 5's 40-pin GPIO header and convert SPI0 data
and clock from 3.3V to 5V for an SK9822/APA102 LED strip.
No high-current LED power passes through this board.

Files
design.svg: electrical schematic and illustrative component placement.
connections.csv: complete connected-net list; all other header pins are NC.
bom.csv: provisional bill of materials.

Electrical design
J1 is a 2x20 female socket fitted on the underside of the PCB.
Physical header pin 19 (GPIO10/MOSI) drives U1 pin 2 (1A).
Physical header pin 23 (GPIO11/SCLK) drives U1 pin 5 (2A).
Physical header pin 6 is common ground.
Pi header pins 1, 2, 4 and 17 are UNCONNECTED. Neither Pi power rail is
connected to the external LED supply. All remaining J1 pins are NC.

J2 is a 2-pin low-current logic-power input: 1 = +5V_LED, 2 = GND.
Power J2 from the SAME regulated 5V supply as the LEDs, with an
appropriately protected small branch. It is not a strip power input.
U1 pin 14 is +5V_LED and pin 7 is GND.
C1 = 100nF between these rails, beside pin 14 with a short ground return.

U1 channels 1 and 2 are enabled by grounding pins 1 and 4 (/OE).
R1 and R2 (10k) pull U1 inputs 2 and 5 to ground when Pi pins are floating.
U1 pin 3 -> R3 (33 ohm) -> J3 pin 1 DATA.
U1 pin 6 -> R4 (33 ohm) -> J3 pin 2 CLOCK.
J3 pin 3 is GND. It deliberately has no +5V output.
R3/R4 are source-series resistors, placed beside U1's output pins.

Unused U1 channels: /OE pins 10 and 13 tied to +5V_LED; input pins 9
and 12 tied to GND; output pins 8 and 11 left NC.

External wiring
J3 DATA -> strip DI; J3 CLOCK -> strip CI; J3 GND -> strip GND.
LED supply +5V/GND -> external power distribution -> strip power taps.
LED ground MUST return to the supply through the external power wiring,
not through J3, this board, or the Pi. J3 ground is a signal reference.
Pi stays powered by USB-C. Use short data/clock wiring, routed with ground.
No chip-select, MISO, ID EEPROM, regulator, or LED power distribution.
This is a GPIO add-on, not a certified HAT or HAT+.

Mechanical concept
Target outline: approximately 65x30mm, two upper mounting holes and an
underside GPIO socket. Placement in design.svg is illustrative only.
Start from the official Raspberry Pi mechanical drawing when creating CAD.
Use a tall socket and matched insulating spacers to clear the Pi cooler.
Verify cooler, case, cable exit, socket pin orientation and solder clearance
with the actual Pi 5 before fixing the board dimensions or ordering.
Place U1, resistors and capacitor on top. Label pin 1 and all connectors.
Prefer keyed connectors. Keep a solid ground plane below the SPI traces.

Bring-up
With power disconnected, check supply polarity and continuity, including
isolation of Pi header 5V/3.3V pins from J2 +5V. Verify U1 orientation.
Enable SPI0 in Raspberry Pi OS, start at 1MHz mode 0, and test dim red,
green and blue on a short strip using SK9822-aware framing.
Check LED supply off/Pi on and Pi off/LED supply on behaviour on hardware.
Pull-downs establish low inputs when undriven; this is not galvanic isolation.
Plug/unplug the board only with BOTH supplies disconnected.

Validation and remaining work
SVG XML and CSV net consistency checked. No KiCad runtime is installed.
No schematic ERC, PCB routing/DRC, Gerbers, thermal assessment, signal
integrity measurement, power-sequence test or mechanical fit test yet.
Connector part numbers, footprints and socket height still need selection.
Do not send this concept to a PCB manufacturer.

Sources
TI SN74AHCT125 datasheet (N = PDIP-14, 7.62mm row spacing):
https://www.ti.com/lit/ds/symlink/sn74ahct125.pdf
Raspberry Pi SPI0 pin mapping:
https://www.raspberrypi.com/documentation/computers/raspberry-pi.html#spi0
Small GPIO-board mechanical reference:
https://datasheets.raspberrypi.com/tv-hat/tv-hat-mechanical-drawing.pdf
SK9822 frame details:
https://www.pololu.com/product/3089
