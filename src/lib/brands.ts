import type { BrandIconName } from './brand-icons';

const PROVIDER_ICONS: Record<string, BrandIconName> = {
  anthropic: 'anthropic',
  openai: 'openai',
  'openai-codex': 'openai',
  'azure-openai-responses': 'azure-color',
  azure: 'azure-color',
  'github-copilot': 'githubcopilot',
  github: 'github',
  opencode: 'opencode',
  'opencode-go': 'opencode',
  google: 'google-color',
  gemini: 'gemini-color',
  'google-gemini-cli': 'geminicli-color',
  'google-vertex': 'vertexai-color',
  'google-antigravity': 'gemini-color',
  openrouter: 'openrouter',
  xai: 'xai',
  groq: 'groq',
  mistral: 'mistral-color',
  deepseek: 'deepseek-color',
  zai: 'zai',
  zhipu: 'zhipu-color',
  qwen: 'qwen-color',
  alibaba: 'qwen-color',
  moonshot: 'kimi-color',
  moonshotai: 'kimi-color',
  kimi: 'kimi-color',
  minimax: 'minimax-color',
  cerebras: 'cerebras-color',
  fireworks: 'fireworks-color',
  together: 'together-color',
  huggingface: 'huggingface-color',
  perplexity: 'perplexity-color',
  cohere: 'cohere-color',
  ollama: 'ollama',
  nvidia: 'nvidia-color',
  'amazon-bedrock': 'bedrock-color',
  bedrock: 'bedrock-color',
  aws: 'aws-color',
};

export function providerIcon(provider: string): BrandIconName | undefined {
  return PROVIDER_ICONS[provider] ?? PROVIDER_ICONS[provider.split(/[-_/]/)[0]];
}

// Most specific first: "gpt-5.3-codex" is OpenAI, "claude-…" is Claude.
const MAKERS: Array<[RegExp, BrandIconName]> = [
  [/claude|opus|sonnet|haiku|fable/, 'claude-color'],
  [/gemini|gemma/, 'gemini-color'],
  [/\bgpt|codex|\bo[134](-|$)|chatgpt/, 'openai'],
  [/grok/, 'grok'],
  [/deepseek/, 'deepseek-color'],
  [/kimi|moonshot/, 'kimi-color'],
  [/glm|zhipu/, 'zhipu-color'],
  [/qwen|qwq/, 'qwen-color'],
  [/mistral|codestral|devstral|magistral/, 'mistral-color'],
  [/minimax/, 'minimax-color'],
  [/longcat/, 'longcat-color'],
  [/llama|\bmeta\b/, 'meta-color'],
  [/nemotron/, 'nvidia-color'],
];

/** The model maker's logo, from the model's id or name; undefined when unknown. */
export function modelIcon(model: { id: string; name?: string }): BrandIconName | undefined {
  const text = `${model.id} ${model.name ?? ''}`.toLowerCase();
  return MAKERS.find(([pattern]) => pattern.test(text))?.[1];
}
