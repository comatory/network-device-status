# Network Device Status

<div style="width: 100%; display: flex; align-items: center; justify-content: center;">
    <img
        alt="Screenshot of the application running in menubar"
        src="https://github.com/comatory/network-device-status/raw/main/doc/screenshot.png"
        width="600px"
        height="auto"
    />
</div>

MacOS menubar application which does few simple things. It indicates internet connectivity by polling 1.1.1.1, but it's main feature is showing which network interface is providing the connectivity. It is useful if you want to be sure that you've connected from LAN to Wi-Fi for example.

## Install

The easiest way is to pick the latest release from [releases page](https://github.com/comatory/network-device-status/releases).
You might get security warnings when starting the app for the first time, open _Privacy & Security_ settings, you should see the application listed there with the button _Open anyway_. Clicking it will allow you to launch it.

## Build

You need Bun runtime and `rustc` installed. Create your own binary by running:

`bun tauri build`

A finder open should open which will allow you to drag the application to your disk.

## Development

Use `bun tauri dev` to launch the application in development mode.
