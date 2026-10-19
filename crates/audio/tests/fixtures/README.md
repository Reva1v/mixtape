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

`sine_mp3.mp3` mimics Bandcamp's MP3 streams and downloads (with an ID3v2 tag in front):

```sh
ffmpeg -f lavfi -i "sine=frequency=440:sample_rate=44100:duration=2" -ac 2 -c:a libmp3lame -b:a 64k -id3v2_version 3 -metadata title=Sine sine_mp3.mp3
```

`sine_hls_aac.m4a` is SoundCloud's HLS AAC as yt-dlp's native HLS downloader joins it (init
segment, then `styp` + `sidx` + `moof` + `mdat` per 0.5 s segment):

```sh
ffmpeg -f lavfi -i "sine=frequency=440:sample_rate=44100:duration=2" -ac 2 -c:a aac -b:a 64k -f dash -seg_duration 0.5 -use_template 1 -use_timeline 0 -init_seg_name init.m4s -media_seg_name 'seg-$Number$.m4s' out.mpd
cat init.m4s seg-1.m4s seg-2.m4s seg-3.m4s seg-4.m4s seg-5.m4s > sine_hls_aac.m4a
```
