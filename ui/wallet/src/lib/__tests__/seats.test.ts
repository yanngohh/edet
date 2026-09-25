/** Whether this member's standing carries one more account. */
import { describe, expect, it } from 'vitest';
import { canSeat, seatsLeft } from '../seats';
import type { OperationBondView } from '../api';

const bond = (seat_reach: number, unit = 10): OperationBondView =>
    ({ encumbered: 0, headroom: 0, unit, free_remaining: 0, saturated_epochs: 0, seat_reach, established: true, seat_slot: false }) as any;

describe('seats', () => {
    it('is the reach over the unit, floored', () => {
        expect(seatsLeft(bond(35))).toBe(3);
        expect(seatsLeft(bond(9.99))).toBe(0);
        expect(seatsLeft(bond(0))).toBe(0);
    });
    it('is unknown, not zero, without a position or a unit', () => {
        expect(seatsLeft(undefined)).toBeNull();
        expect(seatsLeft(null)).toBeNull();
        expect(seatsLeft(bond(35, 0))).toBeNull();
    });
    it('blocks only a position KNOWN to carry nothing', () => {
        expect(canSeat(bond(10))).toBe(true);
        expect(canSeat(bond(9.5))).toBe(false);
        expect(canSeat(undefined)).toBe(true);
    });
});
