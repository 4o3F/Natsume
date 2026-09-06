export interface PreparationPreview {
  candidate_id: string;
  preview_token: string;
}

export function createPreparationStore(signal: AbortSignal) {
  let preview: PreparationPreview | null = null;
  return {
    get(): PreparationPreview | null {
      return preview;
    },
    set(value: PreparationPreview): void {
      signal.throwIfAborted();
      preview = { ...value };
    },
    clear(): void {
      preview = null;
    },
  };
}
