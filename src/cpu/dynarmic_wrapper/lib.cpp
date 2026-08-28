/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <array>
#include <memory>
#include <optional>

#include "dynarmic/interface/A32/a32.h"
#include "dynarmic/interface/A32/config.h"
#include "dynarmic/interface/A32/coprocessor.h"
#include "dynarmic/interface/exclusive_monitor.h"

namespace touchHLE::cpu {

using VAddr = std::uint32_t;

extern "C" {
struct touchHLE_Mem;

std::uint8_t touchHLE_cpu_read_u8(
    touchHLE_Mem *mem, VAddr addr, bool *error);

std::uint16_t touchHLE_cpu_read_u16(
    touchHLE_Mem *mem, VAddr addr, bool *error);

std::uint32_t touchHLE_cpu_read_u32(
    touchHLE_Mem *mem, VAddr addr, bool *error);

std::uint64_t touchHLE_cpu_read_u64(
    touchHLE_Mem *mem, VAddr addr, bool *error);

bool touchHLE_cpu_write_u8(
    touchHLE_Mem *mem, VAddr addr, std::uint8_t value);

bool touchHLE_cpu_write_u16(
    touchHLE_Mem *mem, VAddr addr, std::uint16_t value);

bool touchHLE_cpu_write_u32(
    touchHLE_Mem *mem, VAddr addr, std::uint32_t value);

bool touchHLE_cpu_write_u64(
    touchHLE_Mem *mem, VAddr addr, std::uint64_t value);

struct touchHLE_DynarmicContext {
    std::array<std::uint32_t, 16> regs;
    std::array<std::uint32_t, 64> extregs;
    std::uint32_t cpsr;
    std::uint32_t fpscr;
};
}

const auto HaltReasonSvc =
    Dynarmic::HaltReason::UserDefined1;

const auto HaltReasonUndefinedInstruction =
    Dynarmic::HaltReason::UserDefined2;

const auto HaltReasonBreakpoint =
    Dynarmic::HaltReason::UserDefined3;

class Environment final : public Dynarmic::A32::UserCallbacks {
public:
    Dynarmic::A32::Jit *cpu = nullptr;
    touchHLE_Mem *mem = nullptr;
    std::uint64_t ticks_remaining;
    uint32_t halting_svc = 0;

private:

    std::uint8_t MemoryRead8(VAddr vaddr) override {
        bool error = false;

        auto value = touchHLE_cpu_read_u8(
            mem, vaddr, &error);

        if (error) {
            std::fprintf(
                stderr,
                "[CPU][MEM][READ8] ERROR addr=0x%08x\n",
                vaddr
            );

            cpu->HaltExecution(
                Dynarmic::HaltReason::MemoryAbort
            );
        }

        return value;
    }

    std::uint16_t MemoryRead16(VAddr vaddr) override {
        bool error = false;

        auto value = touchHLE_cpu_read_u16(
            mem, vaddr, &error);

        if (error) {
            std::fprintf(
                stderr,
                "[CPU][MEM][READ16] ERROR addr=0x%08x\n",
                vaddr
            );

            cpu->HaltExecution(
                Dynarmic::HaltReason::MemoryAbort
            );
        }

        return value;
    }

    std::uint32_t MemoryRead32(VAddr vaddr) override {
        bool error = false;

        auto value = touchHLE_cpu_read_u32(
            mem, vaddr, &error);

        if (error) {
            std::fprintf(
                stderr,
                "[CPU][MEM][READ32] ERROR addr=0x%08x\n",
                vaddr
            );

            cpu->HaltExecution(
                Dynarmic::HaltReason::MemoryAbort
            );
        }

        return value;
    }

    std::uint64_t MemoryRead64(VAddr vaddr) override {
        bool error = false;

        auto value = touchHLE_cpu_read_u64(
            mem, vaddr, &error);

        if (error) {
            std::fprintf(
                stderr,
                "[CPU][MEM][READ64] ERROR addr=0x%08x\n",
                vaddr
            );

            cpu->HaltExecution(
                Dynarmic::HaltReason::MemoryAbort
            );
        }

        return value;
    }

    std::optional<std::uint32_t>
    MemoryReadCode(VAddr vaddr) override {
        bool error = false;

        auto value = touchHLE_cpu_read_u32(
            mem, vaddr, &error
        );

        if (error) {
            std::fprintf(
                stderr,
                "[CPU][CODE] ERROR addr=0x%08x\n",
                vaddr
            );

            return std::nullopt;
        }

        return value;
    }

    void MemoryWrite8(
        VAddr vaddr,
        std::uint8_t value
    ) override {
        if (touchHLE_cpu_write_u8(
                mem, vaddr, value)) {

            std::fprintf(
                stderr,
                "[CPU][MEM][WRITE8] ERROR addr=0x%08x value=0x%02x\n",
                vaddr,
                value
            );

            cpu->HaltExecution(
                Dynarmic::HaltReason::MemoryAbort
            );
        }
    }

    void MemoryWrite16(
        VAddr vaddr,
        std::uint16_t value
    ) override {
        if (touchHLE_cpu_write_u16(
                mem, vaddr, value)) {

            std::fprintf(
                stderr,
                "[CPU][MEM][WRITE16] ERROR addr=0x%08x value=0x%04x\n",
                vaddr,
                value
            );

            cpu->HaltExecution(
                Dynarmic::HaltReason::MemoryAbort
            );
        }
    }

    void MemoryWrite32(
        VAddr vaddr,
        std::uint32_t value
    ) override {
        if (touchHLE_cpu_write_u32(
                mem, vaddr, value)) {

            std::fprintf(
                stderr,
                "[CPU][MEM][WRITE32] ERROR addr=0x%08x value=0x%08x\n",
                vaddr,
                value
            );

            cpu->HaltExecution(
                Dynarmic::HaltReason::MemoryAbort
            );
        }
    }

    void MemoryWrite64(
        VAddr vaddr,
        std::uint64_t value
    ) override {
        if (touchHLE_cpu_write_u64(
                mem, vaddr, value)) {

            std::fprintf(
                stderr,
                "[CPU][MEM][WRITE64] ERROR addr=0x%08x value=0x%016llx\n",
                vaddr,
                static_cast<unsigned long long>(value)
            );

            cpu->HaltExecution(
                Dynarmic::HaltReason::MemoryAbort
            );
        }
    }

    bool MemoryWriteExclusive8(
        VAddr,
        std::uint8_t,
        std::uint8_t
    ) override {
        std::fprintf(
            stderr,
            "[CPU][EXCLUSIVE] MemoryWriteExclusive8 TODO\n"
        );

        abort();
    }

    bool MemoryWriteExclusive16(
        VAddr,
        std::uint16_t,
        std::uint16_t
    ) override {
        std::fprintf(
            stderr,
            "[CPU][EXCLUSIVE] MemoryWriteExclusive16 TODO\n"
        );

        abort();
    }

    bool MemoryWriteExclusive32(
        VAddr addr,
        std::uint32_t value,
        std::uint32_t expected
    ) override {
        auto current = MemoryRead32(addr);

        if (current != expected) {
            std::fprintf(
                stderr,
                "[CPU][EXCLUSIVE32] mismatch "
                "addr=0x%08x expected=0x%08x got=0x%08x\n",
                addr,
                expected,
                current
            );

            abort();
        }

        MemoryWrite32(addr, value);

        return true;
    }

    bool MemoryWriteExclusive64(
        VAddr,
        std::uint64_t,
        std::uint64_t
    ) override {
        std::fprintf(
            stderr,
            "[CPU][EXCLUSIVE] MemoryWriteExclusive64 TODO\n"
        );

        abort();
    }

    void InterpreterFallback(
        std::uint32_t pc,
        size_t num_instructions
    ) override {
        std::fprintf(
            stderr,
            "[CPU][INTERPRETER] fallback "
            "pc=0x%08x instructions=%zu\n",
            pc,
            num_instructions
        );

        abort();
    }

    void CallSVC(std::uint32_t svc) override {
        halting_svc = svc;

        std::fprintf(
            stderr,
            "[CPU][SVC] svc=0x%08x\n",
            svc
        );

        cpu->HaltExecution(HaltReasonSvc);
    }

    void ExceptionRaised(
        VAddr pc,
        Dynarmic::A32::Exception exception
    ) override {

        std::fprintf(
            stderr,
            "[CPU][EXCEPTION] pc=0x%08x "
            "exception=%u\n",
            pc,
            unsigned(exception)
        );

        if (exception ==
            Dynarmic::A32::Exception::NoExecuteFault) {

            std::fprintf(
                stderr,
                "[CPU][EXCEPTION] NoExecuteFault "
                "pc=0x%08x\n",
                pc
            );

            cpu->HaltExecution(
                Dynarmic::HaltReason::MemoryAbort
            );

        } else if (
            exception ==
            Dynarmic::A32::Exception::UndefinedInstruction
        ) {

            std::fprintf(
                stderr,
                "[CPU][EXCEPTION] UndefinedInstruction "
                "pc=0x%08x\n",
                pc
            );

            cpu->HaltExecution(
                HaltReasonUndefinedInstruction
            );

        } else if (
            exception ==
            Dynarmic::A32::Exception::Breakpoint
        ) {

            std::fprintf(
                stderr,
                "[CPU][EXCEPTION] Breakpoint "
                "pc=0x%08x\n",
                pc
            );

            cpu->HaltExecution(
                HaltReasonBreakpoint
            );

        } else {

            std::fprintf(
                stderr,
                "[CPU][EXCEPTION] UNEXPECTED "
                "exception=%u pc=0x%08x\n",
                unsigned(exception),
                pc
            );

            abort();
        }
    }

    void AddTicks(
        std::uint64_t ticks
    ) override {
        if (ticks > ticks_remaining) {
            ticks_remaining = 0;
            return;
        }

        ticks_remaining -= ticks;
    }

    std::uint64_t GetTicksRemaining()
        override {
        return ticks_remaining;
    }
};

class ArmDynarmicCP15
    : public Dynarmic::A32::Coprocessor {

    std::uint32_t addr = 0;

public:
    using CoprocReg =
        Dynarmic::A32::CoprocReg;

    CallbackOrAccessOneWord CompileSendOneWord(
        bool two,
        unsigned opc1,
        CoprocReg CRn,
        CoprocReg CRm,
        unsigned opc2
    ) override {

        if (
            !two &&
            CRn == CoprocReg::C7 &&
            opc1 == 0 &&
            CRm == CoprocReg::C10 &&
            opc2 == 5
        ) {

            std::fprintf(
                stderr,
                "[CPU][CP15] DMB operation\n"
            );

            return &addr;
        }

        std::fprintf(
            stderr,
            "[CPU][CP15] Unhandled SendOneWord "
            "two=%d opc1=%u CRn=%u CRm=%u opc2=%u\n",
            two,
            opc1,
            unsigned(CRn),
            unsigned(CRm),
            opc2
        );

        return CallbackOrAccessOneWord{};
    }

    std::optional<Callback>
    CompileInternalOperation(
        bool,
        unsigned,
        CoprocReg,
        CoprocReg,
        CoprocReg,
        unsigned
    ) override {
        return std::nullopt;
    }

    CallbackOrAccessTwoWords
    CompileSendTwoWords(
        bool,
        unsigned,
        CoprocReg
    ) override {
        return CallbackOrAccessTwoWords{};
    }

    CallbackOrAccessOneWord
    CompileGetOneWord(
        bool,
        unsigned,
        CoprocReg,
        CoprocReg,
        unsigned
    ) override {
        return CallbackOrAccessOneWord{};
    }

    CallbackOrAccessTwoWords
    CompileGetTwoWords(
        bool,
        unsigned,
        CoprocReg
    ) override {
        return CallbackOrAccessTwoWords{};
    }

    std::optional<Callback>
    CompileLoadWords(
        bool,
        bool,
        CoprocReg,
        std::optional<std::uint8_t>
    ) override {
        return std::nullopt;
    }

    std::optional<Callback>
    CompileStoreWords(
        bool,
        bool,
        CoprocReg,
        std::optional<std::uint8_t>
    ) override {
        return std::nullopt;
    }
};

class DynarmicWrapper {

    Environment env;

    std::unique_ptr<
        Dynarmic::A32::Jit
    > cpu;

    std::unique_ptr<
        Dynarmic::ExclusiveMonitor
    > mon;

    std::array<
        std::uint8_t *,
        Dynarmic::A32::UserConfig::
            NUM_PAGE_TABLE_ENTRIES
    > page_table;

public:

    DynarmicWrapper(
        void *direct_memory_access_ptr,
        size_t null_page_count
    ) {

        std::fprintf(
            stderr,
            "[CPU][INIT] Creating Dynarmic wrapper\n"
        );

        Dynarmic::A32::UserConfig user_config;

        user_config.callbacks = &env;

        user_config.coprocessors[15] =
            std::make_shared<ArmDynarmicCP15>();

        mon =
            std::make_unique<
                Dynarmic::ExclusiveMonitor
            >(1);

        user_config.global_monitor =
            mon.get();

#ifndef NDEBUG
        user_config.check_halt_on_memory_access =
            true;
#endif

        if (direct_memory_access_ptr) {

            page_table.fill(
                (std::uint8_t *)
                    direct_memory_access_ptr
            );

            static_assert(
                1 <<
                Dynarmic::A32::UserConfig::PAGE_BITS
                == 0x1000
            );

            if (
                null_page_count >
                page_table.size()
            ) {

                std::fprintf(
                    stderr,
                    "[CPU][INIT] Too many null pages: "
                    "%zu / %zu\n",
                    null_page_count,
                    page_table.size()
                );

                abort();
            }

            for (
                size_t i = 0;
                i < null_page_count;
                i++
            ) {
                page_table[i] = nullptr;
            }

            user_config.page_table =
                &page_table;

            user_config.absolute_offset_page_table =
                true;

            std::fprintf(
                stderr,
                "[CPU][INIT] Direct memory=%p "
                "null_pages=%zu\n",
                direct_memory_access_ptr,
                null_page_count
            );
        }

        cpu =
            std::make_unique<
                Dynarmic::A32::Jit
            >(user_config);

        env.cpu = cpu.get();

        std::fprintf(
            stderr,
            "[CPU][INIT] Dynarmic initialized\n"
        );
    }

    const std::uint32_t *
    regs() const {
        return &cpu->Regs().front();
    }

    std::uint32_t *
    regs() {
        return &cpu->Regs().front();
    }

    std::uint32_t
    cpsr() const {
        return cpu->Cpsr();
    }

    void set_cpsr(
        std::uint32_t cpsr
    ) {
        cpu->SetCpsr(cpsr);
    }

    void invalidate_cache_range(
        VAddr start,
        std::uint32_t size
    ) {
        std::fprintf(
            stderr,
            "[CPU][CACHE] invalidate "
            "start=0x%08x size=%u\n",
            start,
            size
        );

        cpu->InvalidateCacheRange(
            start,
            size
        );
    }

    void swap_context(
        touchHLE_DynarmicContext *context
    ) {

        touchHLE_DynarmicContext tmp = {
            cpu->Regs(),
            cpu->ExtRegs(),
            cpu->Cpsr(),
            cpu->Fpscr()
        };

        cpu->Regs() =
            context->regs;

        cpu->ExtRegs() =
            context->extregs;

        cpu->SetCpsr(
            context->cpsr
        );

        cpu->SetFpscr(
            context->fpscr
        );

        *context = tmp;
    }

    std::int32_t run_or_step(
        touchHLE_Mem *mem,
        std::uint64_t *ticks
    ) {

        env.mem = mem;

        Dynarmic::HaltReason hr;

        if (ticks) {

            env.ticks_remaining =
                *ticks;

            hr = cpu->Run();

        } else {

            hr = cpu->Step();
        }

        std::fprintf(
            stderr,
            "[CPU][HALT] reason=0x%x "
            "pc=0x%08x cpsr=0x%08x "
            "thumb=%d\n",
            unsigned(hr),
            cpu->Regs()[Dynarmic::A32::Reg::PC],
            cpu->Cpsr(),
            (cpu->Cpsr() &
             Dynarmic::A32::CPSR::T) != 0
        );

        std::int32_t res;

        if (
            (!hr && ticks) ||
            (
                hr ==
                Dynarmic::HaltReason::Step &&
                !ticks
            )
        ) {

            res = -1;

        } else if (
            Dynarmic::Has(
                hr,
                Dynarmic::HaltReason::MemoryAbort
            )
        ) {

            std::fprintf(
                stderr,
                "[CPU][RESULT] MemoryAbort\n"
            );

            res = -2;

        } else if (
            Dynarmic::Has(
                hr,
                HaltReasonUndefinedInstruction
            )
        ) {

            auto pc =
                cpu->Regs()[
                    Dynarmic::A32::Reg::PC
                ];

            auto lr =
                cpu->Regs()[
                    Dynarmic::A32::Reg::LR
                ];

            std::fprintf(
                stderr,
                "[CPU][RESULT] UndefinedInstruction "
                "PC=0x%08x LR=0x%08x "
                "CPSR=0x%08x\n",
                pc,
                lr,
                cpu->Cpsr()
            );

            res = -3;

        } else if (
            Dynarmic::Has(
                hr,
                HaltReasonBreakpoint
            )
        ) {

            std::fprintf(
                stderr,
                "[CPU][RESULT] Breakpoint\n"
            );

            res = -4;

        } else if (
            Dynarmic::Has(
                hr,
                HaltReasonSvc
            )
        ) {

            std::fprintf(
                stderr,
                "[CPU][RESULT] SVC=0x%08x\n",
                env.halting_svc
            );

            res =
                std::int32_t(
                    env.halting_svc
                );

        } else {

            std::fprintf(
                stderr,
                "[CPU][RESULT] UNHANDLED "
                "halt reason=0x%x\n",
                unsigned(hr)
            );

            abort();
        }

        env.mem = nullptr;

        if (ticks) {
            *ticks =
                env.ticks_remaining;
        }

        return res;
    }
};

extern "C" {

DynarmicWrapper *
touchHLE_DynarmicWrapper_new(
    void *direct_memory_access_ptr,
    size_t null_page_count
) {
    return new DynarmicWrapper(
        direct_memory_access_ptr,
        null_page_count
    );
}

void
touchHLE_DynarmicWrapper_delete(
    DynarmicWrapper *cpu
) {
    delete cpu;
}

const std::uint32_t *
touchHLE_DynarmicWrapper_regs_const(
    const DynarmicWrapper *cpu
) {
    return cpu->regs();
}

std::uint32_t *
touchHLE_DynarmicWrapper_regs_mut(
    DynarmicWrapper *cpu
) {
    return cpu->regs();
}

std::uint32_t
touchHLE_DynarmicWrapper_cpsr(
    const DynarmicWrapper *cpu
) {
    return cpu->cpsr();
}

void
touchHLE_DynarmicWrapper_set_cpsr(
    DynarmicWrapper *cpu,
    std::uint32_t cpsr
) {
    cpu->set_cpsr(cpsr);
}

void
touchHLE_DynarmicWrapper_swap_context(
    DynarmicWrapper *cpu,
    touchHLE_DynarmicContext *context
) {
    cpu->swap_context(context);
}

void
touchHLE_DynarmicWrapper_invalidate_cache_range(
    DynarmicWrapper *cpu,
    VAddr start,
    std::uint32_t size
) {
    cpu->invalidate_cache_range(
        start,
        size
    );
}

std::int32_t
touchHLE_DynarmicWrapper_run_or_step(
    DynarmicWrapper *cpu,
    touchHLE_Mem *mem,
    std::uint64_t *ticks
) {
    return cpu->run_or_step(
        mem,
        ticks
    );
}

}

} // namespace touchHLE::cpu
