# Network Device Status

MacOS menubar application which does few simple things. It indicates internet connectivity by polling 1.1.1.1, but it's main feature is showing which network interface is providing the connectivity. It is useful if you want to be sure that you've connected from LAN to Wi-Fi for example.

## Install

You need Bun runtime installed. Create your own binary by running:

`bun tauri build`

A finder open should open which will allow you to drag the application to your disk.

## Development

Use `bun tauri dev` to launch the application in development mode.
