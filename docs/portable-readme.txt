TokWatch 0.1.0 - Windows x64 preview

A native Windows 11 system tray monitor for your remaining Codex allowance.
Project: https://github.com/MeysamResan/mxs-tokwatch

REQUIREMENTS
Windows 11 x64 and a compatible native Codex installation signed in with a
ChatGPT account. Rust is not required to run this executable.

GET STARTED
1. Extract the ZIP into a permanent folder.
2. Run TokWatch.exe. Find its icon beside the clock or in the tray overflow.
3. Hover over the icon for details, or click to keep the panel open.
4. If needed, use Connect to Codex, or sign in through your Codex installation
   and then choose Refresh. Use Choose Codex executable if discovery fails.

Right-click the icon to choose the usage window, refresh interval, or optional
Start with Windows. Leave the executable in its intended folder before enabling
startup. If you move it later, disable and re-enable startup from the new folder.
Use Exit in the tray menu to stop the application.

For simulated data that does not query your account, exit any running TokWatch
instance and run from PowerShell: .\TokWatch.exe --demo

READINGS AND PRIVACY
TokWatch shows allowance percentages, not an exact token balance. It also shows
reset times, available full resets, account plan, and credits when Codex reports
them. Authentication stays with the official Codex helper. Monitoring does not
send model prompts, purchase credits, or redeem resets.

Settings and a minimized usage cache are stored in %LOCALAPPDATA%\TokWatch.
The usage cache excludes authentication tokens and account email addresses.

KNOWN LIMITATIONS
This is an unsigned preview build for Windows 11 x64. Windows may display an
unknown-publisher warning. There is no installer or automatic updater.
Fresh browser sign-in and broader interactive, accessibility, and long-running
behavior still need manual validation. If browser sign-in fails, sign in through
Codex and refresh TokWatch. Missing account fields are shown as unavailable.

REMOVAL
Disable Start with Windows, choose Exit, and delete the application folder.
Optionally delete %LOCALAPPDATA%\TokWatch to remove its settings and usage cache.
Your Codex installation and sign-in are separate.

LICENSE
MIT. See the included LICENSE file. TokWatch is an independent project and is
not an official OpenAI application.
