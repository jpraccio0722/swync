# Getting Started

The simplest way to make a sound, simply type:

`sin(440)`

and press **CMD + ,** (comma) or press play.  
**CMD + .** stops the audio

`sin(440)*0.1`

multiplying a signal will reduce the volume (amplitude).

Pressing play (CMD+,) replaces currently running audio immediately.


```swync
let freq = 220
(sin(freq) + sin(freq*2)) * 0.25
```

A swync file will evaluate this code alone. But let's make this something we can use in more complex patterns.

We can create a function that will be used to make things more interesting. A function can simply run code and return a number or list. A function can also return a signal and function as an instrument definition.

Let's mix it up with a different oscilator:

```swync
fn synth(freq) {
  saw(freq) + saw(freq*2) * 0.5
}
```

Alone this dosn't do anything. But add below:

`synth(220)`

```swync
fn synth(freq) {
  saw(freq) + saw(freq*2) * 0.5
}

synth(220)
```

And presto, sound. Not yet inspriring, but sound.  
This will play continuously until you stop.

