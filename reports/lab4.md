# Chapter 6

# Q1

root inode作为唯一个实际存在的文件夹，可以来定位文件inode（通过遍历root inode的数据块，对每一个`DirEntry`检查）之后来读写文件内容

损坏后会导致所有对文件的读写可能会有问题，因为除非是已经打开的文件（fd_table中保存了`Inode`）其余，要根据一个文件名打开文件只能通过遍历`root inode`的数据块，如果损坏了难以正确定位到对应的inode

# Chapter 7

# Q1

连接两个应用，`history | grep xxx`, `cat xxx | wc`

# Q2

在内核中维护一个宏读/写管道/总线，根据pid来为不同进程进行分发

