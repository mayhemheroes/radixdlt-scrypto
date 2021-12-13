initSidebarItems({"enum":[["ApplicationError",""],["BorrowedSubstate",""],["DropFailure",""],["HeapRENode",""],["KernelError",""],["ModuleError",""],["NativeSubstateRef",""],["RENodePointer",""],["RENodeRef",""],["RENodeRefMut",""],["RejectionError","Represents an error which causes a tranasction to be rejected."],["RuntimeError","Represents an error when executing a transaction."],["StateTrackError",""],["Substate",""],["SysCallInput",""],["SysCallOutput",""],["TrackError",""]],"fn":[["insert_non_root_nodes",""],["verify_stored_value_update",""]],"macro":[["log",""],["trace",""]],"struct":[["AppStateTrack","Keeps track of state changes that may be rolled back according to transaction status"],["AuthModule",""],["BaseStateTrack","Keeps track of state changes that that are non-reversible, such as fee payments"],["CallFrame","A call frame is the basic unit that forms a transaction call stack, which keeps track of the owned objects by this function."],["CostingModule",""],["ExecutionTrace",""],["ExecutionTraceReceipt",""],["HeapRootRENode",""],["Kernel",""],["LoggerModule",""],["NativeInterpreter",""],["NopWasmRuntime","A `Nop` runtime accepts any external function calls by doing nothing and returning void."],["REActor",""],["RENodeProperties",""],["RadixEngineWasmRuntime","A glue between system api (call frame and track abstraction) and WASM."],["ResourceChange",""],["SubstateProperties",""],["Track","Enforces borrow semantics of global objects and collects transaction-wise side effects, such as logs and events."],["TrackReceipt",""]],"trait":[["Module",""],["SystemApi",""]]});