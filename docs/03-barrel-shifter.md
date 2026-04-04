# Milestone 3: The Barrel Shifter

## What is a Barrel Shifter?

ARM's secret weapon: the second operand of any ALU instruction can be **shifted for free** — no extra instruction needed. This means `ADD R2, R0, R1, LSL #2` (add R0 + R1×4) is a single instruction.

## Shift Types

```
LSL (Logical Shift Left)     — bits move left, 0s fill from right
  00000011 LSL #2  →  00001100     (multiply by 4)

LSR (Logical Shift Right)    — bits move right, 0s fill from left
  00001100 LSR #2  →  00000011     (unsigned divide by 4)

ASR (Arithmetic Shift Right) — bits move right, SIGN BIT fills from left
  11110000 ASR #2  →  11111100     (signed divide by 4, stays negative!)

ROR (Rotate Right)           — bits that fall off the right wrap to the left
  00001101 ROR #2  →  01000011     (circular rotation)
```

## Why It Matters

This is how ARM does fast multiplication by small constants without a multiply instruction:

| Want | Use | Because |
|------|-----|---------|
| R0 × 2 | MOV R1, R0, LSL #1 | Shift left 1 = ×2 |
| R0 × 4 | MOV R1, R0, LSL #2 | Shift left 2 = ×4 |
| R0 × 5 | ADD R1, R0, R0, LSL #2 | R0 + R0×4 = R0×5 |
| R0 ÷ 8 | MOV R1, R0, LSR #3 | Shift right 3 = ÷8 |

## Carry Out

Every shift produces a **carry-out bit** — the last bit that was shifted away. For logical operations (AND, ORR, MOV, etc.), this carry-out updates the C flag. For arithmetic operations (ADD, SUB), the C flag comes from the ALU instead.

## ASR vs LSR: Signed vs Unsigned

This is subtle but important:
- **LSR** (logical) fills with 0s → treats the number as unsigned
- **ASR** (arithmetic) fills with the sign bit → preserves negative numbers

```
0xFF000000 LSR #24 = 0x000000FF  (255 — treated as positive)
0xFF000000 ASR #24 = 0xFFFFFFFF  (-1 — sign preserved)
```

## Overflow Flag

The V (overflow) flag detects **signed arithmetic errors**:
- Adding two positive numbers and getting negative → overflow
- Adding two negative numbers and getting positive → overflow
- `0x7FFFFFFF + 1 = 0x80000000` — max positive becomes negative!

The formula: `overflow = (op1_sign == op2_sign) && (result_sign != op1_sign)`

## Encoding in Instructions

The shift is encoded in the bottom 12 bits of the instruction (operand2):

```
For immediate shift:
  bits 11-7:  shift amount (0-31)
  bits 6-5:   shift type (00=LSL, 01=LSR, 10=ASR, 11=ROR)
  bit 4:      0 (immediate shift)
  bits 3-0:   Rm (register to shift)

For register shift:
  bits 11-8:  Rs (register holding shift amount)
  bit 7:      0
  bits 6-5:   shift type
  bit 4:      1 (register shift)
  bits 3-0:   Rm (register to shift)
```

## Bug We Hit

When encoding `MOV R0, R0, LSR #1`, the shift type bits (6-5) must be `01` (LSR). We accidentally encoded `00` (LSL) — a common mistake when hand-assembling. The encoding is `0xE1A000A0`, not `0xE1A00020`.

## What's Next

With the barrel shifter, our CPU handles real ARM patterns. Next steps:
- THUMB mode (16-bit compressed instructions, used by most GBA games)
- Or loading a test ROM to see what instruction patterns we're still missing
