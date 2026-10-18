/**
 * Owner: conversations. Each line's persona by slug, kept across visits so the index shows
 * a loading line only the first time a slug is seen. The list route (ConversationView)
 * carries no persona, so the index reads each detail after the list lands; the detail page
 * seeds it too, from every read and save. Memory only, never persisted.
 */
export const personaCache = new Map<string, string>();
