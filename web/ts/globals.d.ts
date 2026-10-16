interface FilePickerType {
  description: string;
  accept: Record<string, string[]>;
}

interface Window {
  showDirectoryPicker?(options: { id: string }): Promise<FileSystemDirectoryHandle>;
  showOpenFilePicker?(options: {
    id: string;
    multiple: boolean;
    types: FilePickerType[];
  }): Promise<FileSystemFileHandle[]>;
  showSaveFilePicker?(options: {
    id: string;
    suggestedName: string;
    types: FilePickerType[];
  }): Promise<FileSystemFileHandle>;
}

interface DataTransferItem {
  getAsFileSystemHandle?(): Promise<FileSystemHandle | null>;
}
