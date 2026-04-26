import axios from 'axios';
import AsyncStorage from '@react-native-async-storage/async-storage';

const BASE_URL = 'http://146.190.134.110:8793/api';
const VOICE_API_URL = 'http://146.190.134.110:8789/api/voice';

// ── Main chat API client ──
const apiClient = axios.create({
  baseURL: BASE_URL,
  timeout: 10000,
  headers: {
    'Content-Type': 'application/json',
  },
});

// Attach JWT token to every request
apiClient.interceptors.request.use(async (config) => {
  const token = await AsyncStorage.getItem('auth_token');
  if (token) {
    config.headers.Authorization = `Bearer ${token}`;
  }
  return config;
});

// ── Voice API client (ACC AgentVoice) ──
const voiceClient = axios.create({
  baseURL: VOICE_API_URL,
  timeout: 30000, // voice requests can be slow
});

// Attach JWT token to voice requests too
voiceClient.interceptors.request.use(async (config) => {
  const token = await AsyncStorage.getItem('auth_token');
  if (token) {
    config.headers.Authorization = `Bearer ${token}`;
  }
  return config;
});

// ──────────────────────────────
//  Chat API methods
// ──────────────────────────────

// Auth
export const login = (username, password) =>
  apiClient.post('/auth/login', { username, password }).then((r) => r.data);

// Channels
export const getChannels = () =>
  apiClient.get('/channels').then((r) => r.data);

// Messages
export const getMessages = (channelId, limit = 50) =>
  apiClient
    .get('/messages', { params: { channel_id: channelId, limit } })
    .then((r) => r.data);

export const postMessage = (channelId, content) =>
  apiClient.post('/messages', { channel_id: channelId, content }).then((r) => r.data);

// Presence
export const getPresence = () =>
  apiClient.get('/presence').then((r) => r.data);

// DMs
export const getDMs = () =>
  apiClient.get('/dms').then((r) => r.data);

// ──────────────────────────────
//  Voice API methods (AgentVoice)
// ──────────────────────────────

/**
 * Send recorded audio to the voice API for speech-to-text transcription.
 * @param {string} audioUri - Local file URI of the recorded audio
 * @returns {Promise<{text: string}>} Transcribed text
 */
export const sendVoiceForSTT = async (audioUri) => {
  const formData = new FormData();

  // Extract filename from URI
  const filename = audioUri.split('/').pop() || 'recording.wav';
  const ext = filename.split('.').pop()?.toLowerCase() || 'wav';
  const mimeTypes = {
    wav: 'audio/wav',
    mp3: 'audio/mpeg',
    m4a: 'audio/m4a',
    aac: 'audio/aac',
    ogg: 'audio/ogg',
    webm: 'audio/webm',
  };

  formData.append('audio', {
    uri: audioUri,
    name: filename,
    type: mimeTypes[ext] || 'audio/wav',
  });

  const response = await voiceClient.post('/stt', formData, {
    headers: {
      'Content-Type': 'multipart/form-data',
    },
  });
  return response.data; // { text: "transcribed text" }
};

/**
 * Request text-to-speech audio from the voice API.
 * @param {string} text - Text to synthesize
 * @param {string} [voice] - Optional voice identifier
 * @returns {Promise<string>} URL/blob URL of the generated audio
 */
export const requestTTS = async (text, voice) => {
  const payload = { text };
  if (voice) {
    payload.voice = voice;
  }

  const response = await voiceClient.post('/tts', payload, {
    responseType: 'blob',
  });

  // On React Native we get an arraybuffer/blob; create a local URL
  // In a browser context this would be URL.createObjectURL(response.data)
  // For React Native, the response URL itself can be used, or we write to cache
  // TODO: For React Native, save blob to filesystem and return local URI
  // For now, return the request URL as a fallback identifier
  if (response.data?.url) {
    return response.data.url;
  }

  // If the API returns the audio inline, construct a data URI or use the response URL
  return `${VOICE_API_URL}/tts?text=${encodeURIComponent(text)}${voice ? `&voice=${encodeURIComponent(voice)}` : ''}`;
};

/**
 * Check the health/status of the voice API.
 * @returns {Promise<object>} Status object from the voice API
 */
export const getVoiceStatus = () =>
  voiceClient.get('/status').then((r) => r.data);

export default apiClient;
