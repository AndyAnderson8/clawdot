# ClawDot brand and theme

Status: Accepted (design guidance, not a requirement). Applies to the logo files in [`assets/brand/`](../../assets/brand) and to the theme in `portal/src/theme.css`.

## The mark

A friendly crab with both claws raised, standing on a low round puck, with a thin ring of light around the puck's top edge. The crab is the "claw": it nods to the idea of a small agent with grabbing hands, and the puck with its glowing ring is a generic round smart-speaker form. Both were drawn from scratch for this project as plain SVG paths (no raster, no fonts embedded, no third-party artwork).

| File | Use | Size |
|---|---|---|
| `assets/brand/clawdot-logo.svg` | Full logo: mark plus the ClawDot wordmark. Adapts to light and dark pages. | 1.7 KB |
| `assets/brand/clawdot-mark.svg` | The mark alone, transparent background. | 1.3 KB |
| `assets/brand/clawdot-icon.svg` | Square app icon / favicon source: the mark on a rounded dark tile. Legible at 32 px. | 1.4 KB |
| `assets/brand/clawdot-mono.svg` | One colour (black, white in dark mode). For stamps, engraving, print, and one-colour contexts. | 1.2 KB |

The wordmark is live text in the system UI font (bold, fixed width through `textLength`), so no font file is shipped. If a fixed outlined wordmark is ever needed for print, convert the text to paths at that time.

## Colour tokens

| Token | Light | Dark | Role |
|---|---|---|---|
| `--bg` | `#f4f8f7` | `#0b1a1f` | Page background |
| `--surface` | `#ffffff` | `#112830` | Panels, inputs |
| `--fg` | `#0e2a33` | `#e8f3f1` | Text (lagoon ink) |
| `--mut` | `#476068` | `#9fb7b4` | Secondary text |
| `--line` | `#cbdad7` | `#24414a` | Dividers |
| `--edge` | `#6f8c88` | `#5e8089` | Control borders |
| `--acc` | `#0b6e63` | `#2ee6c5` | Buttons, links, the "Dot" |
| `--on-acc` | `#ffffff` | `#062019` | Text on `--acc` |
| `--bad` | `#b42318` | `#ff9a8f` | Warnings, destructive actions |
| `--bad-bg` / `--on-bad` | `#b42318` / `#fff` | `#a32115` / `#fff` | Open-mode banner |
| `--ring` | `#2ee6c5` | same | The light ring (logo only) |
| `--shell` | `#f2644f` | same | The crab (logo, banner accent) |
| `--band` / `--on-band` / `--band-mut` | `#0e2a33` / `#e8f3f1` / `#9fb7b4` | same | The portal header and tab band (7.0:1 for the muted text, 11:1 for the ring on it) |

Type scale (ratio 1.25, system fonts only): 0.875, 1, 1.25, 1.563, 1.953 rem. Spacing: 0.25, 0.5, 0.75, 1, 1.5, 2.5 rem. Radii: 8 px (controls), 14 px (panels), pill (buttons, echoing the ring).

### Contrast (WCAG 2.x, computed from the hex values above)

| Pair | Light | Dark | Needs |
|---|---|---|---|
| Text on page background | 14.04 | 15.67 | 4.5 |
| Text on surface | 15.03 | 13.52 | 4.5 |
| Secondary text on background | 6.25 | 8.38 | 4.5 |
| Secondary text on surface | 6.69 | 7.23 | 4.5 |
| Button text on `--acc` | 6.13 | 10.78 | 4.5 |
| `--acc` as link/heading colour on background | 5.73 | 11.20 | 4.5 |
| Danger text on background | 6.14 | 8.69 | 4.5 |
| Banner text on banner | 6.57 | 7.54 | 4.5 |
| Control border on background | 3.39 | 4.17 | 3 (non-text) |
| Focus ring on background | 5.73 | 11.20 | 3 (non-text) |
| "Dot" wordmark on white (logo, light) | 6.13 | n/a | 4.5 |

All pairs meet WCAG AA. Re-run the check whenever a token changes.

## Motion, focus, and input

- Focus: every focusable element gets a 3 px `--focus` outline with a 2 px offset (`:focus-visible`).
- Motion is minimal: one fade-in of the logo header on the setup and sign-in views, and a brightness change on buttons. `prefers-reduced-motion: reduce` turns all animation and transitions off.
- Tap targets are at least 2.75 rem high (phone use). Text is never below 0.875 rem.

## Usage

- Clear space: keep at least the width of one claw (about one quarter of the mark's width) free on every side.
- Minimum size: mark 24 px wide, full logo 96 px wide. Below that use the icon.
- Dark pages: use `clawdot-logo.svg` (it switches itself) or the icon tile. On a photo or a busy background use the mono file.
- Do: use the files as supplied; keep the proportions; keep the ring aqua.
- Do not: recolour the crab, add shadows or outlines, rotate or stretch the mark, put the mark inside another shape other than the supplied tile, or animate the claws.

## Why it avoids third-party marks

The mark is a crab on a generic round puck with a ring of light. It does not use Amazon, Alexa or Echo marks, wordmarks, or the characteristic Echo light-bar styling (a segmented blue ring with a directional highlight), and the product name never uses "Echo" or "Alexa" as a brand element. The crab idea is a general mascot theme; this drawing was made for ClawDot and is not traced from, or modelled on, any other project's artwork. The reference device is mentioned in documentation only as hardware (nominative use). Keep it that way: do not add vendor marks, and review any new brand asset against this list.

## Screenshots

The web UI on a phone-width window (headless Chrome against the real binary): [setup, dark](screenshots/ui-setup-dark.png), [setup, light](screenshots/ui-setup-light.png), [sign-in, dark](screenshots/ui-login-dark.png), [open-mode banner, light](screenshots/ui-open-banner-light.png). Tabbed portal (dark, 500 px wide): [Home](screenshots/portal-home-dark.png), [Reminders](screenshots/portal-reminders-dark.png), [Agents](screenshots/portal-agents-dark.png), [Tools](screenshots/portal-tools-dark.png), [Connections](screenshots/portal-connections-dark.png), [Webhooks](screenshots/portal-webhooks-dark.png), [Skills](screenshots/portal-skills-dark.png). Layout and tab names: [portal.md](portal.md).
