#ifndef UNICODE
#define UNICODE
#endif
#define _UNICODE
#include <windows.h>
#include <wchar.h>
#ifndef FIXTURE_VERSION
#define FIXTURE_VERSION L"1"
#endif
static LRESULT CALLBACK wndproc(HWND window, UINT message, WPARAM wparam, LPARAM lparam) {
    if (message == WM_COMMAND && wparam == 106) { DestroyWindow(window); return 0; }
    if (message == WM_DESTROY) { PostQuitMessage(0); return 0; }
    return DefWindowProcW(window, message, wparam, lparam);
}
int WINAPI wWinMain(HINSTANCE instance, HINSTANCE previous, PWSTR command, int show) {
#ifdef FIXTURE_EXIT_CODE
    return FIXTURE_EXIT_CODE;
#endif
    if (wcsstr(command, L"--uninstall-integration")) {
        wchar_t path[32768];
        DWORD length = GetEnvironmentVariableW(L"TOKWATCH_INSTALLER_FIXTURE_DATA", path, 32000);
        if (!length || length >= 32000) return 1;
        wcscat(path, L"\\cleanup-called.txt");
        HANDLE file = CreateFileW(path, GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, NULL);
        if (file == INVALID_HANDLE_VALUE) return 1;
        DWORD written; WriteFile(file, "cleanup invoked", 15, &written, NULL); CloseHandle(file);
        return 0;
    }
    HANDLE mutex = CreateMutexW(NULL, FALSE, L"Local\\TokWatch.Installer.Verification.v1");
    if (!mutex || GetLastError() == ERROR_ALREADY_EXISTS) return 1;
    WNDCLASSW wc = {0}; wc.lpfnWndProc = wndproc; wc.hInstance = instance;
    wc.lpszClassName = L"TokWatch.Installer.Verification.Window";
    if (!RegisterClassW(&wc)) return 1;
    HWND window = CreateWindowW(wc.lpszClassName, FIXTURE_VERSION, WS_OVERLAPPED, 0, 0, 10, 10, NULL, NULL, instance, NULL);
    if (!window) return 1;
    MSG message; while (GetMessageW(&message, NULL, 0, 0) > 0) { TranslateMessage(&message); DispatchMessageW(&message); }
    CloseHandle(mutex); return 0;
}
