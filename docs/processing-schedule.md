# The processing schedule

The worker computer is usually also the computer that you use each day.
Transcription and diarization use almost all of the GPU. The schedule tells the
worker which hours of the week it can use for the heavy stages of the pipeline.

A typical result: the queue becomes empty while you are away, and the GPU stays
free while you use the computer.

## The stages that wait

The `transcode`, `diarize`, `transcribe`, `merge`, `embed` and `summarize`
stages of the pipeline wait for a window.

## The stages that continue

Two functions always continue:

- **Live transcription**: a recording that is in progress gets its transcript
  immediately.
- **Uploads**: the app always sends the audio to the server.

Live transcription uses only 15 seconds of audio for each job, and you started
that recording. Recordings are safe while the worker waits, because the audio
and the jobs stay in the database until the window opens.

## How to set the hours

1. In the app, select `Settings`.
2. Select `Processing schedule`.
3. Set `Limit processing to a schedule` to on.
4. For each day, set the day to on or off.
5. For a day that is on, select the start time and then the end time.
6. To give the same hours to Tuesday, Wednesday, Thursday and Friday, select
   `Copy Monday to Tue-Fri`.
7. Select `Save schedule`.

The times are the local time of the server, not the local time of the phone.
You can see the time of the server on the schedule screen.

### Hours that continue after midnight

An end time that is at or before the start time continues into the next day.
For example, a start time of 22:00 and an end time of 06:00 gives 8 hours. The
screen shows `+1d` for a window of this type.

Two windows that touch become one window. A window that ends at 06:00 and a
window that starts at 06:00 on the next day give one continuous window.

## Manual control

The `Right now` card has two buttons:

- `Process now` starts the heavy stages although the schedule is closed. Use it
  when you want the worker to start the jobs in the queue immediately.
- `Pause now` stops the heavy stages although the schedule is open. Use it when
  you come to the computer in your usual hours.

Each button gives you a list of times: 30 minutes, 1 hour, 2 hours or 4 hours.
The maximum is 24 hours. At the end of that time, the schedule controls the
worker again.

To go back to the schedule before that time, select `Back to the schedule`.

## A window that closes during a stage

A stage that is in progress does not stop immediately. The worker gives it the
time in the `Grace period` field, and then puts the job in the queue again for
the next window. The default is 10 minutes.

The job keeps its position and its attempt count. A closed window is not a
failure, thus a job cannot become `failed` because the schedule stopped it.

One limit is important: the worker can only stop a stage at a point where the
stage waits. A model operation that is in progress on the GPU continues to its
end. The maximum time is thus the time in the `Grace period` field plus the
length of one model operation.

## The status on the screen

The `Backlog` card shows three counts:

- `Recordings waiting` — recordings with a minimum of one job that waits for
  the schedule.
- `Jobs queued` — all jobs in the queue.
- `Running` — jobs that a worker holds at this time.

The card above the counts shows the status of the server: `Processing` or
`Paused`, the cause, and the time until the next change.

## Where the schedule is

The schedule is on the server, in the `app_settings` table (migration `0009`).
It is not on the phone. Two phones thus always show the same schedule, and the
schedule stays correct after you install the app again.

The default is off. If there is no schedule, the server operates the jobs at
all times.

## The API

| Method | Route | Function |
|---|---|---|
| `GET` | `/processing-schedule` | The schedule, the status and the counts |
| `PUT` | `/processing-schedule` | Replace the days |
| `POST` | `/processing-schedule/override` | `run`, `pause` or `clear` |

The routes use the device token, not the update token.

A `PUT` does not remove a `Process now` or `Pause now` command that is active.
The server keeps that command through the write.

Times in the API are minutes after midnight, from `0` to `1440`. The `days`
array must have 7 items, and the first item is Monday.

After a write, the server sends a notification to the workers. An idle worker
reads the new schedule in less than 20 seconds.
