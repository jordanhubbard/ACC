import React, { useEffect, useState, useRef, useCallback } from 'react';
import {
  View,
  Text,
  FlatList,
  TextInput,
  TouchableOpacity,
  StyleSheet,
  ActivityIndicator,
  KeyboardAvoidingView,
  Platform,
  Alert,
} from 'react-native';
import { NativeStackNavigationProp } from '@react-navigation/native-stack';
import { RouteProp } from '@react-navigation/native';
import { getMessages, postMessage, sendVoiceForSTT, requestTTS } from '../api/client';
import { getUser } from '../store/auth';
import wsClient from '../ws/client';
import MessageBubble from '../components/MessageBubble';
import VoiceButton from '../components/VoiceButton';
import { RootStackParamList } from '../../App';

type Props = {
  navigation: NativeStackNavigationProp<RootStackParamList, 'Messages'>;
  route: RouteProp<RootStackParamList, 'Messages'>;
};

interface Message {
  id: string | number;
  content: string;
  author: string;
  ts: string;
  audioUrl?: string;
}

export default function MessagesScreen({ navigation, route }: Props) {
  const { channelId, channelName } = route.params;
  const [messages, setMessages] = useState<Message[]>([]);
  const [loading, setLoading] = useState(true);
  const [sending, setSending] = useState(false);
  const [draft, setDraft] = useState('');
  const [currentUser, setCurrentUser] = useState<string>('');
  const [isVoiceMode, setIsVoiceMode] = useState(false);
  const [voiceTranscript, setVoiceTranscript] = useState('');
  const [ttsLoading, setTtsLoading] = useState<string | number | null>(null);
  const flatListRef = useRef<FlatList>(null);

  // Load initial messages
  const fetchMessages = useCallback(async () => {
    try {
      const data = await getMessages(channelId);
      setMessages(Array.isArray(data) ? data : []);
    } catch (err: any) {
      Alert.alert('Error', err?.message || 'Failed to load messages');
    } finally {
      setLoading(false);
    }
  }, [channelId]);

  useEffect(() => {
    navigation.setOptions({ title: `#${channelName}` });

    // Get current user for "isOwn" bubble coloring
    getUser().then((u) => {
      if (u?.username) setCurrentUser(u.username);
    });

    fetchMessages();

    // Connect WebSocket and listen for new messages
    wsClient.connect(channelId);
    const unsubscribe = wsClient.on('message', (data: any) => {
      // Filter to only messages for this channel
      if (data?.channel_id === channelId || data?.channel_id === Number(channelId)) {
        const msg: Message = {
          id: data.id ?? Date.now(),
          content: data.content,
          author: data.author,
          ts: data.ts ?? new Date().toISOString(),
        };
        setMessages((prev) => {
          // Deduplicate by id
          if (prev.some((m) => m.id === msg.id)) return prev;
          return [...prev, msg];
        });
      }
    });

    return () => {
      unsubscribe();
      wsClient.disconnect();
    };
  }, [channelId, channelName, fetchMessages, navigation]);

  // Scroll to bottom when messages change
  useEffect(() => {
    if (messages.length > 0) {
      setTimeout(() => flatListRef.current?.scrollToEnd({ animated: true }), 100);
    }
  }, [messages]);

  const handleSend = async () => {
    const content = draft.trim();
    if (!content) return;
    setDraft('');
    setVoiceTranscript('');
    setSending(true);
    try {
      await postMessage(channelId, content);
      // Optimistic: if WS doesn't echo back, re-fetch
      const data = await getMessages(channelId);
      setMessages(Array.isArray(data) ? data : []);
    } catch (err: any) {
      Alert.alert('Error', err?.message || 'Failed to send message');
      setDraft(content); // restore draft on failure
    } finally {
      setSending(false);
    }
  };

  // Voice recording completed — transcribe via STT
  const handleRecordingComplete = useCallback(
    async (audioUri: string) => {
      setIsVoiceMode(true);
      try {
        const result = await sendVoiceForSTT(audioUri);
        if (result?.text) {
          setVoiceTranscript(result.text);
          setDraft((prev) => (prev ? `${prev} ${result.text}` : result.text));
        } else {
          Alert.alert('Voice', 'Could not transcribe audio. Please try again.');
        }
      } catch (err: any) {
        console.error('STT error:', err);
        Alert.alert(
          'Voice Error',
          err?.message || 'Failed to transcribe voice recording',
        );
      } finally {
        setIsVoiceMode(false);
      }
    },
    [],
  );

  // TTS: speak an agent message
  const handleTTS = useCallback(async (message: Message) => {
    if (ttsLoading) return; // prevent double-tap
    setTtsLoading(message.id);
    try {
      const audioUrl = await requestTTS(message.content);

      // Attach audio URL to the message for inline playback
      setMessages((prev) =>
        prev.map((m) =>
          m.id === message.id ? { ...m, audioUrl } : m,
        ),
      );

      // TODO: Auto-play the audio using expo-av Sound
      // const { sound } = await Audio.Sound.createAsync({ uri: audioUrl });
      // await sound.playAsync();
    } catch (err: any) {
      console.error('TTS error:', err);
      Alert.alert('TTS Error', err?.message || 'Failed to generate speech');
    } finally {
      setTtsLoading(null);
    }
  }, [ttsLoading]);

  const renderMessage = useCallback(
    ({ item }: { item: Message }) => {
      const isOwn = item.author === currentUser;
      return (
        <View>
          <MessageBubble message={item} isOwn={isOwn} />
          {/* Speaker icon on non-own (agent) messages for TTS */}
          {!isOwn && (
            <TouchableOpacity
              style={styles.ttsButton}
              onPress={() => handleTTS(item)}
              disabled={ttsLoading === item.id}
              activeOpacity={0.6}>
              {ttsLoading === item.id ? (
                <ActivityIndicator color="#e94560" size="small" />
              ) : (
                <Text style={styles.ttsIcon}>🔊</Text>
              )}
            </TouchableOpacity>
          )}
        </View>
      );
    },
    [currentUser, handleTTS, ttsLoading],
  );

  return (
    <KeyboardAvoidingView
      style={styles.container}
      behavior={Platform.OS === 'ios' ? 'padding' : undefined}
      keyboardVerticalOffset={90}>
      {loading ? (
        <ActivityIndicator style={styles.loader} color="#e94560" size="large" />
      ) : (
        <FlatList
          ref={flatListRef}
          data={messages}
          keyExtractor={(item) => String(item.id)}
          renderItem={renderMessage}
          contentContainerStyle={styles.messageList}
          ListEmptyComponent={
            <Text style={styles.empty}>No messages yet. Say hello!</Text>
          }
        />
      )}

      {/* Voice transcription indicator */}
      {isVoiceMode && (
        <View style={styles.voiceIndicator}>
          <ActivityIndicator color="#e94560" size="small" />
          <Text style={styles.voiceIndicatorText}>Transcribing voice...</Text>
        </View>
      )}

      {/* Composer */}
      <View style={styles.composer}>
        <TextInput
          style={styles.input}
          placeholder={
            voiceTranscript
              ? 'Voice transcribed — edit or send'
              : `Message #${channelName}`
          }
          placeholderTextColor="#888"
          value={draft}
          onChangeText={(text) => {
            setDraft(text);
            if (voiceTranscript && text !== voiceTranscript) {
              setVoiceTranscript('');
            }
          }}
          multiline
          maxLength={2000}
        />

        {/* Voice button — between input and send */}
        <VoiceButton
          onRecordingComplete={handleRecordingComplete}
          disabled={sending || isVoiceMode}
        />

        <TouchableOpacity
          style={[styles.sendBtn, (!draft.trim() || sending) && styles.sendBtnDisabled]}
          onPress={handleSend}
          disabled={!draft.trim() || sending}>
          {sending ? (
            <ActivityIndicator color="#fff" size="small" />
          ) : (
            <Text style={styles.sendText}>Send</Text>
          )}
        </TouchableOpacity>
      </View>
    </KeyboardAvoidingView>
  );
}

const styles = StyleSheet.create({
  container: { flex: 1, backgroundColor: '#1a1a2e' },
  loader: { marginTop: 60 },
  messageList: { paddingVertical: 12, flexGrow: 1 },
  empty: { textAlign: 'center', color: '#888', marginTop: 60, fontSize: 15 },

  // Voice transcription banner
  voiceIndicator: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'center',
    paddingVertical: 6,
    backgroundColor: 'rgba(233, 69, 96, 0.1)',
    borderTopWidth: 1,
    borderTopColor: '#e94560',
  },
  voiceIndicatorText: {
    color: '#e94560',
    fontSize: 13,
    marginLeft: 8,
    fontWeight: '600',
  },

  // Composer
  composer: {
    flexDirection: 'row',
    alignItems: 'flex-end',
    paddingHorizontal: 12,
    paddingVertical: 8,
    borderTopWidth: 1,
    borderTopColor: '#0f3460',
    backgroundColor: '#16213e',
  },
  input: {
    flex: 1,
    backgroundColor: '#1a1a2e',
    borderRadius: 20,
    paddingHorizontal: 16,
    paddingVertical: 10,
    fontSize: 15,
    color: '#fff',
    maxHeight: 120,
    borderWidth: 1,
    borderColor: '#0f3460',
  },
  sendBtn: {
    marginLeft: 8,
    backgroundColor: '#e94560',
    borderRadius: 20,
    paddingHorizontal: 18,
    paddingVertical: 10,
    alignItems: 'center',
    justifyContent: 'center',
    minWidth: 64,
  },
  sendBtnDisabled: { opacity: 0.5 },
  sendText: { color: '#fff', fontWeight: '700', fontSize: 15 },

  // TTS speaker button on agent messages
  ttsButton: {
    marginLeft: 52,
    marginBottom: 4,
    paddingHorizontal: 8,
    paddingVertical: 2,
    alignSelf: 'flex-start',
  },
  ttsIcon: {
    fontSize: 16,
  },
});
