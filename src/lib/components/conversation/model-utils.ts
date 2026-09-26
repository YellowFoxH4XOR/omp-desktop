import type { ModelInfo } from '../../types';

/** Display names for Pi provider ids; unknown ids are title-cased. */
const PROVIDER_LABELS: Record<string, string> = {
  'opencode-go': 'OpenCode Go',
  opencode: 'OpenCode Zen',
  'openai-codex': 'OpenAI Codex',
  openai: 'OpenAI',
  'azure-openai-responses': 'Azure OpenAI',
  anthropic: 'Anthropic',
  google: 'Google Gemini',
  'google-vertex': 'Google Vertex',
  'google-gemini-cli': 'Gemini CLI',
  'github-copilot': 'GitHub Copilot',
  openrouter: 'OpenRouter',
  'amazon-bedrock': 'Amazon Bedrock',
  xai: 'xAI',
  groq: 'Groq',
  cerebras: 'Cerebras',
  mistral: 'Mistral',
  deepseek: 'DeepSeek',
  zai: 'Z.ai',
  huggingface: 'Hugging Face',
  'vercel-ai-gateway': 'Vercel AI Gateway',
};

export function providerLabel(provider: string): string {
  return (
    PROVIDER_LABELS[provider] ??
    provider
      .split(/[-_]/)
      .filter(Boolean)
      .map((part) => part[0].toUpperCase() + part.slice(1))
      .join(' ')
  );
}

export function modelKey(model: Pick<ModelInfo, 'provider' | 'id'>): string {
  return `${model.provider}/${model.id}`;
}

/** 262144 → "262K", 1048576 → "1M", 128000 → "128K". */
export function formatTokens(tokens: number | undefined): string | undefined {
  if (!tokens || tokens <= 0) return undefined;
  if (tokens >= 1_000_000) {
    const millions = tokens / 1_048_576 >= 0.98 && tokens % 1_048_576 === 0 ? tokens / 1_048_576 : tokens / 1_000_000;
    return `${Number(millions.toFixed(millions >= 10 ? 0 : 1))}M`;
  }
  return `${Math.round(tokens / 1000)}K`;
}

export function formatCost(cost: ModelInfo['cost']): string | undefined {
  if (!cost) return undefined;
  if (cost.input === 0 && cost.output === 0) return 'Free';
  const money = (value: number) => (value >= 10 ? value.toFixed(0) : value.toFixed(2).replace(/\.?0+$/, ''));
  return `$${money(cost.input)} / $${money(cost.output)}`;
}

/** Every whitespace-separated term must match the name, id, or provider. */
export function matchesModel(model: ModelInfo, query: string): boolean {
  const haystack = `${model.name} ${model.id} ${model.provider} ${providerLabel(model.provider)}`.toLowerCase();
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((term) => haystack.includes(term));
}
