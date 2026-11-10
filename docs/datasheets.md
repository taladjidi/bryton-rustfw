# The S510's parts and their datasheets

The parts on the Rider S510's board, where each was identified, and the vendor document this repository cites for
it. The PDFs are not in the repository (their licences forbid redistribution): `tools/datasheets.py` fetches them into
`target/datasheets/` and checks each against the copy cited here. ST's site refuses scripts; for it the tool prints the
link to open in a browser. The facts this firmware relies on go into [BOARD.md](../BOARD.md), in our own words,
citing the document and its table or section.

The board's FCC filing is FCC ID [YDM-CA2404](https://fccid.io/YDM-CA2404) (Bryton, granted 2024-10-17). Its internal
photos are public; its schematics, block diagram and operational description are held confidential, so the wiring is
not in it.

## Identified

| Function | Part | Identified from | Document |
|---|---|---|---|
| MCU | ST STM32H750IBK6: Cortex-M7, 128 KB flash, UFBGA176+25 | FCC internal photos (the owner's reading) | [DS12556 Rev 8](https://www.st.com/resource/en/datasheet/stm32h750ib.pdf) (January 2026; by hand). Its reference manual is RM0433, not collected yet |
| SDRAM | Alliance Memory AS4C32M16MS-6BIN: 512 Mbit mobile SDR, 32M x 16, 166 MHz, industrial (-40 to 85 C) | FCC internal photos (the owner's reading) | [AS4C32M16MS rev 1.0](https://www.alliancememory.com/wp-content/uploads/AS4C32M16MS-6BIN_512M-LOW-POWER-SDRAM_AS4C32M16MS-7BCN__AS4C32M16MS-6BIN-rev1.0-June-2016.pdf) (June 2016, preliminary) |
| USB high-speed PHY | Microchip (SMSC) USB3320, ULPI | FCC internal photos (the owner's reading); its ULPI IDs read on the unit (BOARD.md 7) | [SMSC USB3320 datasheet rev 1.0](https://datasheet4u.com/pdf/737354/USB3320.pdf) (2009-07-14, 82 pages; a mirror: Microchip's [data brief](https://ww1.microchip.com/downloads/en/DeviceDoc/3320db.pdf) has the features only) |
| BLE / ANT radio | Nordic nRF52832 | FCC internal photos (the owner's reading); its SoftDevice and RAM size (BOARD.md 11) | [nRF52832 Product Specification v1.9](https://mm.digikey.com/Volume0/opasdata/d220001/medias/docus/8881/4823_nRF52832_PS_v1.9.pdf) |
| GNSS | u-blox M10 (ROM SPG 5.10), the module not known | UBX-MON-VER on the unit (BOARD.md 13) | [u-blox M10 SPG 5.10 interface description, UBX-21035062](https://content.u-blox.com/sites/default/files/u-blox-M10-SPG-5.10_InterfaceDescription_UBX-21035062.pdf) |
| Display controller | Sitronix ST7789 class, in a 240 x 320 module made for Bryton | its command set (BOARD.md 6) | [Sitronix ST7789V datasheet V1.3](https://newhavendisplay.com/content/datasheets/ST7789V.pdf) (2014/03, 316 pages; also the panel model's source, src/sim/panel_model.rs). The unit's die may be the newer ST7789VW, whose faster serial write is what 50 MHz needs (BOARD.md 6) |
| Touch controller | Sitronix ST1624-N32C: mutual capacitive, up to 5 fingers, I2C | the owner's reading of the board; its 5 contacts (register 0x3f) read on the unit (BOARD.md 17) | [Sitronix ST1624 datasheet v1.1](https://www.crystalfontz.com/controllers/uploaded/Sitronix_ST1624_Datasheet_v1.1.pdf) (2014-10-22, 13 pages: pins, supplies, currents, I2C timing; the registers are in Sitronix's Touch IC Protocol A v2.7, BOARD.md 17) |
| Battery charger | ETA Solutions ETA6003: 2.5 A switching Li-ion charger with power path, QFN3x3-16 | the owner's reading of the board | [ETA6003 v2.5.1](https://e2e.ti.com/cfs-file/__key/communityserver-discussions-components-files/196/ETA6003-v2.5.1.pdf) (7 pages; hosted on TI's E2E forum) |
| Barometer | ST LPS22HH | WHO_AM_I on the unit (BOARD.md 16) | not collected yet |
| Accelerometer | ST LIS2DH12 / LIS3DH class | WHO_AM_I on the unit (BOARD.md 16) | not collected yet |
| Battery | 3.7 V, 1200 mAh, 4.4 Wh cell | its label in the FCC internal photos | |

## Not identified yet

The eMMC, the light sensor (I2C4, BOARD.md 16), the regulators and the buzzer's amplifier (BOARD.md 14).
