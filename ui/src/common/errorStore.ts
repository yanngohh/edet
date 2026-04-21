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
        
        // Auto-dismiss after 10 seconds (increased from 5s — error messages with
        // protocol error codes like "EC200019: You already have an open trial..." need
        // more reading time, especially for non-native speakers using localised text).
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
