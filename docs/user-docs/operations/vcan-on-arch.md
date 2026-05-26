# Persistent `vcan0` on Arch Linux

This guide shows how to make a `vcan0` interface persist across reboots
on Arch Linux while keeping NetworkManager in place for normal network
interfaces.

It replaces the manual setup command:

```bash
sudo modprobe vcan
sudo ip link add dev vcan0 type vcan
sudo ip link set up vcan0
```

## Recommendation

Use `systemd-networkd` to create the virtual CAN device and bring it up
at boot. Keep NetworkManager enabled for Ethernet and Wi-Fi.

This is the cleanest split because:

- `vcan` is a virtual kernel device, not a normal NetworkManager
  connection profile.
- `systemd-networkd` natively supports `.netdev` files for virtual
  devices such as `vcan`.
- You can run `systemd-networkd` only for `vcan0` without handing your
  physical interfaces away from NetworkManager.

## How It Fits Together

- `NetworkManager` continues to manage your regular interfaces.
- `systemd-networkd` creates `vcan0` during boot from files in
  `/etc/systemd/network/`.
- The kernel may autoload the `vcan` module automatically. If it does
  not, load it persistently through `/etc/modules-load.d/`.

## 1. Create The `vcan` Device Definition

Create `/etc/systemd/network/20-vcan0.netdev`:

```ini
[NetDev]
Name=vcan0
Kind=vcan
```

This tells `systemd-networkd` to create a virtual device named
`vcan0`.

## 2. Match The Interface So It Comes Up

Create `/etc/systemd/network/20-vcan0.network`:

```ini
[Match]
Name=vcan0
```

For a `vcan` interface, this match file is enough for `systemd-networkd`
to notice and manage the device.

## 3. Load The Kernel Module At Boot If Needed

If `vcan0` does not appear after reboot, add:

`/etc/modules-load.d/can.conf`

```text
vcan
```

Many systems autoload the module when the device is created, so this
file is optional.

## 4. Enable `systemd-networkd`

Enable and start it:

```bash
sudo systemctl enable systemd-networkd.service
sudo systemctl start systemd-networkd.service
```

You can keep `NetworkManager.service` enabled at the same time.

## Important Notes With NetworkManager

- Do not create broad `systemd-networkd` `.network` files that match
  `en*`, `eth*`, `wlan*`, or other physical interfaces unless you
  intentionally want `systemd-networkd` to manage them.
- With only the `vcan0` files above, `systemd-networkd` should be
  limited to this virtual interface.
- You do not need a NetworkManager connection profile for `vcan0`.

## Verify After Reboot

Check that the interface exists:

```bash
ip link show vcan0
```

Check how `systemd-networkd` sees it:

```bash
networkctl status vcan0
```

Check whether the module is loaded:

```bash
lsmod | grep vcan
```

Expected result:

- `vcan0` exists immediately after boot
- the interface is up
- you no longer need to run `modprobe` or `ip link add` manually

## Alternative: NetworkManager Dispatcher Script

If you want the setup tied to NetworkManager events instead of boot-time
device creation, the script location is:

`/etc/NetworkManager/dispatcher.d/`

That approach works, but it is less direct than the `systemd-networkd`
method because it uses an event hook to create a device that
`systemd-networkd` already knows how to declare natively.
