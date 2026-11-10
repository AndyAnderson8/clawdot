# Forgot the administrator password

ClawDot has one administrator password for the web page. If you lose it, there are four ways back in. Use the first one that applies to you. None of them touches your timers, notes, webhooks or settings.

| You have | Use |
|---|---|
| The recovery code from setup | [Option 1](#1-the-recovery-code) |
| A speaker with an action button, or a PC running the device program | [Option 2](#2-the-action-button-or-the-pc-key) |
| A PC or server where you can open a terminal | [Option 3](#3-the-command-line) |
| An Echo Dot with no way to run commands | [Option 1 or 2; otherwise Option 4](#4-an-echo-dot-with-no-code-and-no-button-hold) |

## 1. The recovery code

When you set your password, the page showed a recovery code once: four groups of five letters and digits, like `ABCDE-FGHJK-MNPQR-STUVW`. It is the only copy; ClawDot keeps just a scrambled form of it.

1. Open the page and choose **Forgot your password?** under the sign-in box.
2. Type the code (capitals or small letters, with or without dashes) and a new password of at least 8 characters.
3. The page signs you in and shows a **new** recovery code. Save it. The old code no longer works, and every other signed-in browser is signed out.

If the page says the code was not accepted, check for a mix-up between `2` and `Z` or `8` and `B`; the code never contains `0`, `1`, `I`, `L` or `O`. Many wrong tries in a row are slowed down on purpose; wait a minute.

Installed before recovery codes existed? Sign in, open **Settings**, and choose **Make a recovery code**. Do this now, while you still know the password.

## 2. The action button, or the PC key

If you cannot find the code but can reach the device:

1. **A speaker with an action button:** hold the button for 10 seconds. You hear a short tone and the light changes. (If you hear a low double beep instead, the speaker was refused; see the table below.)
2. **The device program on a PC:** type `admin` and press Enter in the console where `clawdot-device` runs.
3. Within 5 minutes, open the ClawDot page from a phone or computer on the same network. The sign-in page now says **The speaker is ready for a new password**. Choose a password.
4. The page signs you in and shows a new recovery code. Save it.

The window opens once, closes after one use or after 5 minutes, and closes if ClawDot restarts. Only pages opened from your own network can use it. After it ends, wait a minute before holding the button again.

If the speaker refuses:

| It means | What to do |
|---|---|
| "This speaker is not trusted" | The speaker reached ClawDot over the network without the satellite token. Set the same token on both sides (`satellite.token_env`), or run the device program on the same machine as ClawDot. |
| "Wait a minute" | A window was used or ended a moment ago. Wait and try again. |
| "No password is set" | The page is in open mode; there is nothing to reset. |

## 3. The command line

On a PC or server, stop ClawDot and run:

```
clawdot --reset-admin --data-dir <your data directory>
```

Start ClawDot again and open the page: the first-run screen asks for a new password. This needs a shell on the machine, which is why Options 1 and 2 exist.

## 4. An Echo Dot with no code and no button hold

A Dot running ClawDot has no screen and often no shell. If you have neither the code nor a working button hold (for example the base image does not report the button), reach the data directory some other way and delete `admin.json` from it, then restart: that is exactly what `--reset-admin` does. How depends on how the Dot was installed; the trial plan for the TECHO5 base image explains where the data directory lives. If you cannot reach the data directory either, the unit needs re-flashing.

## What this protects against

- Someone guessing the code: it carries about 99 bits and wrong tries are rate limited, so guessing is not practical.
- Someone on your network pressing buttons for you: they would have to be at the speaker for 10 seconds, and the speaker announces it; the window is private-network only, short, and single use.
- Someone who reads the data directory: they get only the scrambled forms, as with the password.
- Keep the code out of notes and chats with the assistant. ClawDot never logs or repeats it.
