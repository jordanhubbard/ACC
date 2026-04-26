import React, { useState, useRef, useCallback } from 'react';
import {
  View,
  Text,
  StyleSheet,
  Animated,
  PanResponder,
  GestureResponderEvent,
  PanResponderGestureState,
} from 'react-native';

interface Props {
  onRecordingComplete: (uri: string) => void;
  disabled?: boolean;
}

/**
 * VoiceButton — press-and-hold microphone button for voice recording.
 *
 * Hold to record, release to send. Slide away (>80px) while holding to cancel.
 *
 * TODO: Integrate real audio recording with expo-av or react-native-audio-recorder-player.
 * Currently mocks recording and returns a simulated file URI.
 */
export default function VoiceButton({ onRecordingComplete, disabled = false }: Props) {
  const [isRecording, setIsRecording] = useState(false);
  const [isCancelled, setIsCancelled] = useState(false);
  const pulseAnim = useRef(new Animated.Value(1)).current;
  const recordingStartTime = useRef<number>(0);
  const cancelled = useRef(false);

  // Start pulsing animation
  const startPulse = useCallback(() => {
    const pulse = Animated.loop(
      Animated.sequence([
        Animated.timing(pulseAnim, {
          toValue: 1.3,
          duration: 600,
          useNativeDriver: true,
        }),
        Animated.timing(pulseAnim, {
          toValue: 1,
          duration: 600,
          useNativeDriver: true,
        }),
      ]),
    );
    pulse.start();
    return pulse;
  }, [pulseAnim]);

  const pulseRef = useRef<Animated.CompositeAnimation | null>(null);

  const panResponder = useRef(
    PanResponder.create({
      onStartShouldSetPanResponder: () => !disabled,
      onMoveShouldSetPanResponder: () => true,

      onPanResponderGrant: (_evt: GestureResponderEvent) => {
        // Start recording
        cancelled.current = false;
        setIsCancelled(false);
        setIsRecording(true);
        recordingStartTime.current = Date.now();
        pulseRef.current = startPulse();

        // TODO: Start real audio recording here
        // Example with expo-av:
        // const recording = new Audio.Recording();
        // await recording.prepareToRecordAsync(Audio.RECORDING_OPTIONS_PRESET_HIGH_QUALITY);
        // await recording.startAsync();
      },

      onPanResponderMove: (
        _evt: GestureResponderEvent,
        gestureState: PanResponderGestureState,
      ) => {
        // If user slides away more than 80px, mark as cancelled
        const distance = Math.sqrt(
          gestureState.dx * gestureState.dx + gestureState.dy * gestureState.dy,
        );
        if (distance > 80) {
          if (!cancelled.current) {
            cancelled.current = true;
            setIsCancelled(true);
          }
        } else {
          if (cancelled.current) {
            cancelled.current = false;
            setIsCancelled(false);
          }
        }
      },

      onPanResponderRelease: () => {
        // Stop recording
        setIsRecording(false);
        setIsCancelled(false);
        pulseRef.current?.stop();
        pulseAnim.setValue(1);

        if (cancelled.current) {
          // Recording cancelled — discard
          cancelled.current = false;
          return;
        }

        const duration = Date.now() - recordingStartTime.current;
        if (duration < 300) {
          // Too short — ignore tap-like touches
          return;
        }

        // TODO: Stop real recording and get the file URI
        // Example with expo-av:
        // await recording.stopAndUnloadAsync();
        // const uri = recording.getURI();

        // MOCK: Simulate a recorded file URI
        const mockUri = `file:///tmp/voice_recording_${Date.now()}.wav`;
        onRecordingComplete(mockUri);
      },

      onPanResponderTerminate: () => {
        // Another responder took over — cancel
        setIsRecording(false);
        setIsCancelled(false);
        cancelled.current = false;
        pulseRef.current?.stop();
        pulseAnim.setValue(1);
      },
    }),
  ).current;

  return (
    <View style={styles.container}>
      {isRecording && (
        <View style={styles.recordingOverlay}>
          <Text style={[styles.recordingText, isCancelled && styles.cancelText]}>
            {isCancelled ? '✕ Release to cancel' : '● Recording...'}
          </Text>
          {!isCancelled && (
            <Text style={styles.slideHint}>↕ Slide away to cancel</Text>
          )}
        </View>
      )}

      <Animated.View
        {...panResponder.panHandlers}
        style={[
          styles.button,
          disabled && styles.buttonDisabled,
          isRecording && !isCancelled && styles.buttonActive,
          isRecording && isCancelled && styles.buttonCancelled,
          isRecording && {
            transform: [{ scale: pulseAnim }],
          },
        ]}>
        <Text style={styles.micIcon}>🎤</Text>
      </Animated.View>
    </View>
  );
}

const styles = StyleSheet.create({
  container: {
    position: 'relative',
    marginLeft: 8,
    alignItems: 'center',
    justifyContent: 'center',
  },
  button: {
    width: 44,
    height: 44,
    borderRadius: 22,
    backgroundColor: '#1a1a2e',
    borderWidth: 2,
    borderColor: '#0f3460',
    alignItems: 'center',
    justifyContent: 'center',
  },
  buttonDisabled: {
    opacity: 0.4,
  },
  buttonActive: {
    backgroundColor: '#e94560',
    borderColor: '#e94560',
  },
  buttonCancelled: {
    backgroundColor: '#444',
    borderColor: '#666',
  },
  micIcon: {
    fontSize: 20,
  },
  recordingOverlay: {
    position: 'absolute',
    bottom: 52,
    right: -20,
    backgroundColor: 'rgba(26, 26, 46, 0.95)',
    borderRadius: 12,
    paddingHorizontal: 14,
    paddingVertical: 8,
    borderWidth: 1,
    borderColor: '#e94560',
    minWidth: 140,
    alignItems: 'center',
  },
  recordingText: {
    color: '#e94560',
    fontSize: 14,
    fontWeight: '700',
  },
  cancelText: {
    color: '#888',
  },
  slideHint: {
    color: 'rgba(255,255,255,0.5)',
    fontSize: 11,
    marginTop: 2,
  },
});
