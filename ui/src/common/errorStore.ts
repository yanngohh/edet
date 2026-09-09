import { writable } from 'svelte/store';

export interface AppError {
    message: string;
    type: 'error' | 'warning';
    id: number;
}

const { subscribe, update } = writable<AppError[]>([]);

let errorId = 0;

export const errorStore = {
    subscribe,
    pushError: (message: string, type: 'error' | 'warning' = 'error') => {
        const id = ++errorId;
        update(errors => [...errors, { message, type, id }]);

        // Auto-dismiss after 10 seconds — messages carrying protocol error
        // codes (e.g. "ET-CAP-001: …") need reading time, especially in
        // localised text.
        setTimeout(() => {
            errorStore.removeError(id);
        }, 10000);
    },
    removeError: (id: number) => {
        update(errors => errors.filter(e => e.id !== id));
    },
    clearErrors: () => {
        update(() => []);
    }
};
