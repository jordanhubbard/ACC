import React, { useState, useRef, useCallback } from 'react';
import { View, Text, TouchableOpacity, StyleSheet, Animated } from 'react-native';

interface Props {
  audioUrl: string;
}

/**
 * AudioPlayback — compact inline audio player for TTS responses.
 *
 * Shows a play/pause button with duration indicator, designed to fit
 * inside a MessageBubble.
 *
 * TODO: Integrate real audio playback with expo-av Sound or react-native-sound.
 * Currently mocks playback state with simulated progress.
 */
export default function AudioPlayback({ audioUrl }: Props) {
  const [isPlaying, setIsPlaying] = useState(false);
  const [progress, setProgress] = useState(0);
  const [duration] = useState('0:04'); // TODO: Read real duration from audio file
  const progressAnim = useRef(new Animated.Value(0)).current;
  const playbackTimer = useRef<ReturnType<typeof setInterval> | null>(null);

  const togglePlayback = useCallback(() => {
    if (isPlaying) {
      // Stop/pause
      setIsPlaying(false);
      if (playbackTimer.current) {
        clearInterval(playbackTimer.current);
        playbackTimer.current = null;
      }
      progressAnim.stopAnimation();

      // TODO: Real pause:
      // await soundRef.current?.pauseAsync();
    } else {
      // Play
      setIsPlaying(true);
      setProgress(0);
      progressAnim.setValue(0);

      // TODO: Real playback:
      // const { sound } = await Audio.Sound.createAsync({ uri: audioUrl });
      // soundRef.current = sound;
      // await sound.playAsync();
      // sound.setOnPlaybackStatusUpdate((status) => { ... });

      // MOCK: Simulate 4-second playback with progress
      Animated.timing(progressAnim, {
        toValue: 1,
        duration: 4000,
        useNativeDriver: false,
      }).start(() => {
        setIsPlaying(false);
        setProgress(0);
        progressAnim.setValue(0);
      });

      let elapsed = 0;
      playbackTimer.current = setInterval(() => {
        elapsed += 100;
        setProgress(elapsed / 4000);
        if (elapsed >= 4000) {
          if (playbackTimer.current) {
            clearInterval(playbackTimer.current);
            playbackTimer.current = null;
          }
        }
      }, 100);
    }
  }, [isPlaying, audioUrl, progressAnim]);

  const progressWidth = progressAnim.interpolate({
    inputRange: [0, 1],
    outputRange: ['0%', '100%'],
  });

  const currentTime = React.useMemo(() => {
    const totalSeconds = 4; // TODO: use real duration
    const current = Math.floor(progress * totalSeconds);
    return `0:${String(current).padStart(2, '0')}`;
  }, [progress]);

  return (
    <View style={styles.container}>
      <TouchableOpacity
        onPress={togglePlayback}
        style={styles.playButton}
        activeOpacity={0.7}>
        <Text style={styles.playIcon}>{isPlaying ? '⏸' : '▶'}</Text>
      </TouchableOpacity>

      <View style={styles.waveformContainer}>
        <View style={styles.waveformTrack}>
          <Animated.View
            style={[styles.waveformProgress, { width: progressWidth }]}
          />
        </View>
        <Text style={styles.timeText}>
          {isPlaying ? currentTime : duration}
        </Text>
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    flexDirection: 'row',
    alignItems: 'center',
    marginTop: 6,
    paddingVertical: 4,
    minWidth: 150,
  },
  playButton: {
    width: 30,
    height: 30,
    borderRadius: 15,
    backgroundColor: 'rgba(255,255,255,0.15)',
    alignItems: 'center',
    justifyContent: 'center',
    marginRight: 8,
  },
  playIcon: {
    fontSize: 14,
    color: '#fff',
  },
  waveformContainer: {
    flex: 1,
    justifyContent: 'center',
  },
  waveformTrack: {
    height: 4,
    backgroundColor: 'rgba(255,255,255,0.2)',
    borderRadius: 2,
    overflow: 'hidden',
  },
  waveformProgress: {
    height: '100%',
    backgroundColor: '#e94560',
    borderRadius: 2,
  },
  timeText: {
    color: 'rgba(255,255,255,0.6)',
    fontSize: 11,
    marginTop: 2,
  },
});
