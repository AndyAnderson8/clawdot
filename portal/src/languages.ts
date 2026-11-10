// The languages a person can speak to the assistant, by name. The codes are the ones speech-to-text services take.
const CODES = [
  "af",
  "ar",
  "az",
  "be",
  "bg",
  "bn",
  "bs",
  "ca",
  "cs",
  "cy",
  "da",
  "de",
  "el",
  "en",
  "es",
  "et",
  "fa",
  "fi",
  "fr",
  "gl",
  "he",
  "hi",
  "hr",
  "hu",
  "hy",
  "id",
  "is",
  "it",
  "ja",
  "ka",
  "kk",
  "ko",
  "lt",
  "lv",
  "mk",
  "mr",
  "ms",
  "ne",
  "nl",
  "no",
  "pl",
  "pt",
  "ro",
  "ru",
  "sk",
  "sl",
  "sr",
  "sv",
  "sw",
  "ta",
  "th",
  "tl",
  "tr",
  "uk",
  "ur",
  "vi",
  "zh",
];

/** "English" for `en`; the code itself if the browser does not know it. */
export function languageName(code: string): string {
  try {
    return new Intl.DisplayNames(["en"], { type: "language" }).of(code) ?? code;
  } catch {
    return code;
  }
}

export interface LanguageOption {
  code: string;
  /** For example "English (en)". */
  label: string;
}

/** Every language by name, in alphabetical order, with the current one included even if it is not on the list. */
export function languageOptions(current = ""): LanguageOption[] {
  const codes = !current || CODES.includes(current) ? CODES : [...CODES, current];
  return codes
    .map((code) => ({ code, name: languageName(code) }))
    .sort((a, b) => a.name.localeCompare(b.name))
    .map(({ code, name }) => ({ code, label: name === code ? code : `${name} (${code})` }));
}
