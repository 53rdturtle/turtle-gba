# Milestone 2: Fetch-Decode-Execute Loop

## The Heartbeat

Every CPU cycle follows the same pattern:

```
FETCH ──► DECODE ──► EXECUTE ──► advance PC ──► repeat
```

1. **Fetch**: Read 4 bytes from the address in PC → that's one ARM instruction
2. **Decode**: Look at specific bits to determine the operation
3. **Execute**: Perform the operation (math, memory access, branch)
4. **Advance PC**: Move PC forward by 4 (to the next instruction)

## ARM Instruction Encoding

Every ARM instruction is exactly 32 bits. The bits encode everything:

```
31-28   27-26   25    24-21   20    19-16   15-12   11-0
cond    type    I     opcode  S     Rn      Rd      operand2
```

### Condition Codes (bits 31-28)

**Every** ARM instruction is conditional — the CPU checks flags before executing.
This is unique to ARM and very powerful. Common conditions:

| Code | Name | Meaning | When |
|------|------|---------|------|
| 0xE | AL | Always | Most instructions use this |
| 0x0 | EQ | Equal | Z flag set |
| 0x1 | NE | Not equal | Z flag clear |
| 0xA | GE | Greater or equal (signed) | N == V |
| 0xB | LT | Less than (signed) | N != V |

Example: `MOVEQ R1, #99` only executes if Z flag is set (previous comparison was equal).

### Instruction Categories

**Bits 27-26** determine the category:

| 27-26 | Category | Examples |
|-------|----------|----------|
| 00 | Data processing (ALU) | MOV, ADD, SUB, CMP, AND, ORR |
| 01 | Memory transfer | LDR (load), STR (store) |
| 10 | Branch | B (jump), BL (function call) |
| 11 | Coprocessor / SWI | System calls (later) |

## Data Processing (ALU Operations)

These do math and logic. The **opcode** field (bits 24-21) selects which:

| Opcode | Name | Operation | Stores result? |
|--------|------|-----------|----------------|
| 0x4 | ADD | Rd = Rn + Op2 | Yes |
| 0x2 | SUB | Rd = Rn - Op2 | Yes |
| 0xD | MOV | Rd = Op2 | Yes |
| 0xA | CMP | Rn - Op2 | No (flags only) |
| 0x0 | AND | Rd = Rn & Op2 | Yes |
| 0xC | ORR | Rd = Rn \| Op2 | Yes |

### Immediate Encoding (the clever trick)

When **bit 25 (I) = 1**, operand2 is an immediate value encoded as:
- Bits 7-0: an 8-bit value (0-255)
- Bits 11-8: a 4-bit rotation amount (multiplied by 2)

The value is `imm8 rotated right by (rot * 2)`. This lets you encode many useful constants in just 12 bits. For example, `0xFF000000` is `0xFF` rotated right by 8.

## Branch Instructions

Jumps to a different address. The offset is:
- 24-bit **signed** number (can jump forward or backward)
- Measured in **words** (multiply by 4 to get byte offset)
- **Sign extension**: if bit 23 is 1, the offset is negative — fill upper bits with 1s

**BL** (Branch with Link) saves the return address in LR first — this is how function calls work.

## Load/Store (LDR/STR)

Move data between registers and memory:
- **LDR R1, [R13, #4]** — load the word at address (R13 + 4) into R1
- **STR R0, [R13, #0]** — store R0's value at address R13

## Key Concept: Everything is Bits

An ARM instruction like `MOV R0, #42` is really just the number `0xE3A0002A`:
```
1110 00 1 1101 0 0000 0000 0000 00101010
AL     I MOV   S=0  Rn=0 Rd=0 rot=0 imm=42
```

The CPU doesn't see "MOV" — it sees a 32-bit number and extracts meaning from bit positions.

## What We Implemented

- `cpu.step()` — the fetch-decode-execute loop
- `cpu.condition_met()` — checks all 16 ARM condition codes
- `cpu.execute_data_processing()` — 16 ALU operations (MOV, ADD, SUB, CMP, etc.)
- `cpu.execute_branch()` — B and BL with signed offset
- `cpu.execute_single_transfer()` — LDR and STR

## What's Next

We have a working CPU that can do math, make decisions, and access memory. Next we could:
- Add shift operations (LSL, LSR, ASR, ROR) for operand2
- Implement THUMB mode (16-bit compressed instructions)
- Start loading and running a real test ROM
