# Print from KHEvents

TapPrint can print jobs that sellers send through khevents.app, from any
phone, tablet or laptop. Only this computer needs the printer. Printing from
a page on this computer (the [HTTP API](api.md)) keeps working as before.

## Connect

1. On khevents.app, an owner or admin opens **Console**, **Printers**,
   **Add a printer**. A code like `K7QM-3XPA` shows. It works once, for 10
   minutes.
2. In TapPrint, open **Websites**. Under **Print from KHEvents**, type the
   code and click **Connect** (or press Enter).
3. The card shows the printer and organizer names and **Online**. On the
   Printers page the printer turns green.

Keep TapPrint running. Turn on **Start at login** in Settings.

## While connected

- TapPrint asks for the next job about every 2 seconds. A job with several
  slips (tickets) prints them one by one, each with a cut.
- **Take print jobs** off pauses this computer. Jobs wait on khevents.app
  for up to 15 minutes; sellers see them waiting.
- **Recent jobs** lists the last 20, with who sent them. The full log, with
  every device, is on the Printers page.
- When khevents.app cannot be reached, TapPrint waits longer each time (up
  to 30 seconds) and says **Offline**. Jobs wait on the server meanwhile.

## What the printer said, and what happens next

| Printer | TapPrint reports | khevents.app |
| --- | --- | --- |
| Printed | `printed` | Done. The pictures are deleted. |
| Out of paper | `paper_out` | Tries again in 10 s, from the slip that did not print. |
| Not reachable (unplugged, off, no USB printer) | `printer_busy` | Tries again in 10 s. |
| Stopped part way through a slip | `maybe_printed` | **Check paper**: a person looks and chooses Mark printed or Print again. Never retried by itself. |
| Picture unusable | `printer_error` | Failed. |

If TapPrint stops while it holds a job (quit, crash, network gone), the
server waits 60 seconds. If no slip was downloaded, the job goes back to
waiting; otherwise it becomes **Check paper**. So a ticket is never printed
twice by accident.

## Disconnect

**Disconnect** (two clicks) forgets the key here and tells khevents.app. On
the Printers page, **Remove** does the same from the other side: TapPrint
then says the printer was removed and forgets the key.

## For developers

The protocol is described in the KHEvents repository,
`docs/runbooks/server-printing.md`. TapPrint's side is `src/station.rs`.

A debug build can use another server:

```sh
TAPPRINT_SERVER=http://127.0.0.1:8000 cargo run
```

A release build always uses `https://khevents.app`.

To check against a running KHEvents with a station code and a waiting job:

```sh
TAPPRINT_E2E_SERVER=http://127.0.0.1:8000 TAPPRINT_E2E_CODE=ABCD-EFGH \
TAPPRINT_E2E_SLIPS=1 cargo test -- --ignored against_a_running_khevents
```

It pairs, prints the job on a stand-in network printer, and disconnects.
