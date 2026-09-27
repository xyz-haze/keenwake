FROM python:3.12-slim
RUN pip install --no-cache-dir --index-url https://download.pytorch.org/whl/cpu torch \
 && pip install --no-cache-dir laya==0.3.20
WORKDIR /app
COPY laya_server.py .
ENV LAYA_HOST=0.0.0.0 LAYA_PORT=8771 HF_HOME=/models
VOLUME /models
EXPOSE 8771
CMD ["python", "laya_server.py"]
