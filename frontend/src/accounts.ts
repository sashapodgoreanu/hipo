/* Local, multi-account profiles for Duckle.
   Each account is a lightweight identity (username + optional avatar) bound to
   its own workspace path, so switching accounts switches the active data /
   pipeline context. Stored only on this device (localStorage), never sent
   anywhere - keeps GDPR exposure at zero and there is no auth/password. */

export interface Account {
    id: string;
    username: string;
    /** Small base64 data URL (~96px). Optional. */
    avatar?: string;
    /** The workspace this account works in - its data/pipeline context. */
    workspacePath?: string;
}

const ACCOUNTS_KEY = 'duckle:v1:accounts';
const ACTIVE_KEY = 'duckle:v1:active-account';

export function loadAccounts(): Account[] {
    try {
        const raw = localStorage.getItem(ACCOUNTS_KEY);
        if (!raw) return [];
        const parsed = JSON.parse(raw);
        return Array.isArray(parsed) ? (parsed as Account[]) : [];
    } catch {
        return [];
    }
}

export function saveAccounts(accounts: Account[]): void {
    try {
        localStorage.setItem(ACCOUNTS_KEY, JSON.stringify(accounts));
    } catch {
        /* quota / disabled - drop silently */
    }
}

export function loadActiveAccountId(): string | null {
    try {
        return localStorage.getItem(ACTIVE_KEY);
    } catch {
        return null;
    }
}

export function saveActiveAccountId(id: string | null): void {
    try {
        if (id) localStorage.setItem(ACTIVE_KEY, id);
        else localStorage.removeItem(ACTIVE_KEY);
    } catch {
        /* ignore */
    }
}

export function newAccountId(): string {
    return 'acc_' + Math.random().toString(36).slice(2, 10) + Date.now().toString(36).slice(-4);
}

/** Up to two uppercase initials for the avatar fallback. */
export function initials(name: string): string {
    const parts = name.trim().split(/\s+/).filter(Boolean);
    if (parts.length === 0) return '?';
    if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
    return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase();
}
