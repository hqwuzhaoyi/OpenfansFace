FROM python:3.11-slim
RUN pip install --no-cache-dir numpy
COPY calibrator.py tplgroups.py /opt/
ENV CAL_PYBIN=python3 CAL_PORT=8921
CMD ["python3", "/opt/calibrator.py"]
