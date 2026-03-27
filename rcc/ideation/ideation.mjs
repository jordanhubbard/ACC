/**
 * rcc/ideation/ideation.mjs — Autonomous project idea generation
 *
 * Calls NVIDIA inference API to generate novel project ideas based on
 * recent queue context and lessons learned.
 *
 * Env:
 *   NVIDIA_API_BASE  — base URL for NVIDIA inference API
 *   NVIDIA_API_KEY   — API key (if unset, returns mock ideas)
 */

const NVIDIA_API_BASE = process.env.NVIDIA_API_BASE || 'https://integrate.api.nvidia.com/v1';
const NVIDIA_API_KEY  = process.env.NVIDIA_API_KEY  || '';
const MODEL           = 'nvidia/azure/anthropic/claude-sonnet-4-6';

const MOCK_IDEAS = [
  {
    title: 'Auto-summarize completed queue items into lessons',
    description: 'After a queue item reaches "completed", extract a short lesson from the result and write it to the lessons ledger automatically.',
    rationale: 'Agents forget context between runs. Automating lesson extraction would compound knowledge over time.',
    difficulty: 'medium',
    tags: ['automation', 'lessons', 'queue'],
  },
  {
    title: 'Agent capability self-reporting dashboard',
    description: 'Each agent publishes a capabilities manifest on startup; RCC aggregates and displays a live capability map.',
    rationale: 'Hard to know which agent can do what without reading each agent\'s source. A live capability registry solves this.',
    difficulty: 'low',
    tags: ['dashboard', 'capabilities', 'observability'],
  },
  {
    title: 'Cross-agent voting on promoted ideas',
    description: 'When an idea item appears in the queue, agents autonomously +1 or -1 based on feasibility scoring using their own context.',
    rationale: 'Surfaces the best ideas without human triage. Peer review without meetings.',
    difficulty: 'high',
    tags: ['voting', 'autonomy', 'coordination'],
  },
];

/**
 * Generate a single project idea using the NVIDIA inference API.
 *
 * @param {object} context
 * @param {Array}  context.recentQueue   — recent queue item titles/descriptions
 * @param {Array}  context.recentLessons — recent lessons learned
 * @param {string} context.agentName     — name of the requesting agent
 * @returns {Promise<{title, description, rationale, difficulty, tags}>}
 */
export async function generateIdea(context) {
  const { recentQueue = [], recentLessons = [], agentName = 'unknown' } = context;

  // Fall back to mocks if no API key configured
  if (!NVIDIA_API_KEY) {
    const mock = MOCK_IDEAS[Math.floor(Math.random() * MOCK_IDEAS.length)];
    return { ...mock };
  }

  const queueSummary = recentQueue.length
    ? recentQueue.slice(0, 10).map(i => `- ${i.title || i}`).join('\n')
    : '(none)';

  const lessonSummary = recentLessons.length
    ? recentLessons.slice(0, 5).map(l => `- ${l.symptom || l.fix || l}`).join('\n')
    : '(none)';

  const systemPrompt = `You are an autonomous software agent named ${agentName} brainstorming novel project ideas for a multi-agent AI system called RemoteCode (RCC). Generate creative, actionable project ideas that improve the system.`;

  const userPrompt = `Based on this context, generate ONE novel project idea.

Recent queue items (what agents have been working on):
${queueSummary}

Recent lessons learned:
${lessonSummary}

Return ONLY valid JSON in this exact format (no markdown, no explanation):
{
  "title": "short descriptive title",
  "description": "2-3 sentence description of what to build",
  "rationale": "why this would be valuable",
  "difficulty": "low|medium|high",
  "tags": ["tag1", "tag2"]
}`;

  const response = await fetch(`${NVIDIA_API_BASE}/chat/completions`, {
    method: 'POST',
    headers: {
      'Content-Type': 'application/json',
      'Authorization': `Bearer ${NVIDIA_API_KEY}`,
    },
    body: JSON.stringify({
      model: MODEL,
      messages: [
        { role: 'system', content: systemPrompt },
        { role: 'user', content: userPrompt },
      ],
      temperature: 0.9,
      max_tokens: 512,
    }),
  });

  if (!response.ok) {
    const errText = await response.text().catch(() => '');
    throw new Error(`NVIDIA API error ${response.status}: ${errText.slice(0, 200)}`);
  }

  const data = await response.json();
  const text = data.choices?.[0]?.message?.content || '';

  // Parse JSON from the response (strip any accidental markdown fences)
  const cleaned = text.replace(/^```(?:json)?\s*/i, '').replace(/\s*```\s*$/, '').trim();
  let idea;
  try {
    idea = JSON.parse(cleaned);
  } catch {
    throw new Error(`Failed to parse idea JSON from model response: ${text.slice(0, 300)}`);
  }

  // Validate and normalise
  return {
    title:       String(idea.title || 'Untitled idea'),
    description: String(idea.description || ''),
    rationale:   String(idea.rationale || ''),
    difficulty:  ['low', 'medium', 'high'].includes(idea.difficulty) ? idea.difficulty : 'medium',
    tags:        Array.isArray(idea.tags) ? idea.tags.map(String) : [],
  };
}
