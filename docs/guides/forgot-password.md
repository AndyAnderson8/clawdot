# Forgot the administrator password

ClawDot has one administrator password for the web page. There is no recovery code and no email reset. If you lose the password, there are three ways back in. Use the first one that applies to you. None of them touches your timers, notes, webhooks or settings.

| You have | Use |
|---|---|
| A speaker whose program reports its button to ClawDot, or a PC running the device program | [Option 1](#1-the-action-button-or-the-pc-key) |
| A PC or server where you can open a terminal | [Option 2](#2-the-command-line) |
| An Echo Dot on the TECHO5 image | [Option 3](#3-an-echo-dot-on-the-techo5-image) |

## 1. The action button, or the PC key

1. **A speaker whose program reports the button to ClawDot:** hold the button for 10 seconds. You hear a short tone and the light changes. (If you hear a low double beep instead, the speaker was refused; see the table below.)
2. **The device program on a PC:** type `admin` and press Enter in the console where `clawdot-device` runs.
3. Within 5 minutes, open the ClawDot page from a phone or computer on the same network. The sign-in page now says **The speaker is ready for a new password**. Choose a password of at least 8 characters.
4. The page signs you in. Every other signed-in browser is signed out.

The window opens once, closes after one use or after 5 minutes, and closes if ClawDot restarts. Only pages opened from your own network can use it. After it ends, wait a minute before holding the button again.

If the speaker refuses:

| It means | What to do |
|---|---|
| "This speaker is not trusted" | The speaker reached ClawDot over the network without the satellite token. Set the same token on both sides (`satellite.token_env`), or run the device program on the same machine as ClawDot. |
| "Wait a minute" | A window was used or ended a moment ago. Wait and try again. |
| "No password is set" | The page is in open mode; there is nothing to reset. |

## 2. The command line

On a PC or server, stop ClawDot and run:

```
clawdot --reset-admin --data-dir <your data directory>
```

Start ClawDot again and open the page: the first-run screen asks for a new password. This needs a shell on the machine, which is why Option 1 exists.

## 3. An Echo Dot on the TECHO5 image

On this image the speaker's own software (`echod`) owns the button and does not tell ClawDot about it, so holding the button does **not** open the reset window. What it does instead: a first hold opens the firmware's own setup page, and holding on switches on Bluetooth pairing (two chimes, three minutes apart). That is not a mistake on the Dot, but it is not a way back in.

The way back in is the USB cable. Plug the Dot into a computer and run, in PowerShell:

```
scripts/dev/dot-reset-admin.ps1 -Port COM5
```

(Use the serial port number the Dot shows up as.) It removes the password, keeping everything else, and restarts ClawDot. About 15 seconds later open the portal and choose a new password; until you do, anyone on your network who opens the page can choose it, so do it straight away.

If you cannot reach the cable, the only other way is to reach the data directory by other means and delete `admin.json` from it, then restart ClawDot. If you cannot reach that either, the unit needs re-flashing.

## What this protects against

- Someone on your network pressing buttons for you: they would have to be at the speaker for 10 seconds, and the speaker announces it; the window is private-network only, short, and single use.
- Someone who reads the data directory: they get only the scrambled form of the password.
- Someone guessing the password: wrong tries are rate limited.
