# Audio test fixtures

Generated with ffmpeg 8.1 (lavfi sine: 1/8 full scale, upmixed to stereo). They mimic what
YouTube Music serves: itag 251 is Opus in WebM at 48 kHz, itag 140 is AAC-LC in fragmented MP4
at 44.1 kHz. `delay_moov` makes ffmpeg write an edit list with the AAC priming (1024 frames)
into the fragmented file.

```sh
ffmpeg -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=2" -ac 2 -c:a libopus -b:a 64k sine_opus.webm
ffmpeg -f lavfi -i "sine=frequency=440:sample_rate=44100:duration=2" -ac 2 -c:a aac -b:a 64k -movflags +frag_keyframe+empty_moov+delay_moov sine_aac.m4a
ffmpeg -f lavfi -i "sine=frequency=660:sample_rate=48000:duration=1" -ac 2 -c:a libopus -b:a 64k sine2_opus.webm
ffmpeg -f lavfi -i "sine=frequency=660:sample_rate=44100:duration=1" -ac 2 -c:a aac -b:a 64k -movflags +frag_keyframe+empty_moov+delay_moov sine2_aac.m4a
```
